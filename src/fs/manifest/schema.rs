use std::sync::OnceLock;

use miette::{Diagnostic, miette};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    adapters::{BoxedPlatform, CloudflareClient, CloudflareDeployment, Platform},
    config::RunSubcommand,
    fs::{
        FileSystem,
        wrangler::{Wrangler, WranglerFile},
    },
};
use miette::Result;

/// The project manifest only needs to be loaded once, so we
/// cache it as a global singleton.
static PROJECT_MANIFEST: OnceLock<Manifest> = OnceLock::new();

/// Return the project's manifest file, or an empty manifest if none
/// is found. Use the globally available cached file.
pub fn project_manifest() -> &'static Manifest {
    PROJECT_MANIFEST.get_or_init(Manifest::load_or_default)
}

#[derive(Error, Debug, Diagnostic)]
#[error(
    "When configuration for Cloudflare is present, no configuration for ingress, monitor, or platform can be provided."
)]
struct CloudflareMutuallyExclusiveConfig;

#[derive(Error, Debug, Diagnostic)]
#[error("No platform config found.")]
struct MissingPlatformConfig;

/// With the expectation that we will likely be making breaking changes,
/// we version the manifest schema (like how Docker Compose files come
/// with a schema version). Ideally, we would expect our manifest format
/// to be stable, but every growing project can expect to make breaking
/// changes during its genesis. Wrapping the manifest in a version enum
/// permits easy upgrades and is simple to program around.
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct Manifest {
    workspace: Option<String>,
    application: Option<String>,
    config: ConfigSection,
}

impl Manifest {
    /// Attempts to read a config file for this project, and
    /// returns an empty manifest file if none is found.
    pub(crate) fn load_or_default() -> Self {
        FileSystem::new().map_or(Self::default(), |fs| {
            fs.project_manifest().unwrap_or_default()
        })
    }

    pub fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }

    pub fn application(&self) -> Option<&str> {
        self.application.as_deref()
    }

    pub(crate) fn load_platform(&self, args: &RunSubcommand) -> Result<BoxedPlatform> {
        self.config.load_platform(args)
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct ConfigSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    monitor: Option<MonitorConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ingress: Option<IngressConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    platform: Option<PlatformConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cloudflare: Option<CloudflareConfig>,
}

impl ConfigSection {
    pub fn monitor(&self) -> Option<&MonitorConfig> {
        self.monitor.as_ref()
    }

    pub fn ingress(&self) -> Option<&IngressConfig> {
        self.ingress.as_ref()
    }

    pub fn platform(&self) -> Option<&PlatformConfig> {
        self.platform.as_ref()
    }

    pub fn cloudflare(&self) -> Option<&CloudflareConfig> {
        self.cloudflare.as_ref()
    }

    pub(crate) fn load_platform(&self, args: &RunSubcommand) -> Result<BoxedPlatform> {
        // Having cloudflare configured is mutually exclusive with having
        // platform configured. Error if both are set.
        match (&self.cloudflare, &self.platform) {
            (Some(_), Some(_)) => Err(CloudflareMutuallyExclusiveConfig.into()),
            (None, None) => Err(MissingPlatformConfig.into()),
            (None, Some(platform)) => platform.load_platform(),
            (Some(cloudflare), None) => cloudflare.load_platform(args),
        }
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorConfig {
    AwsCloudwatch(AwsCloudwatch),
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self::AwsCloudwatch(AwsCloudwatch::default())
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct AwsCloudwatch {}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum IngressConfig {
    AwsApiGateway(AwsApiGatewayConfig),
}

impl Default for IngressConfig {
    fn default() -> Self {
        Self::AwsApiGateway(AwsApiGatewayConfig::default())
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct AwsApiGatewayConfig {
    stage_name: String,
    gateway_name: String,
    resource_path: String,
    resource_method: String,
    region: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum PlatformConfig {
    AwsLambda(AwsLambdaConfig),
}

impl PlatformConfig {
    pub fn load_platform(&self) -> Result<BoxedPlatform> {
        match self {
            PlatformConfig::AwsLambda(aws_lambda_config) => aws_lambda_config.load_platform(),
        }
    }
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self::AwsLambda(AwsLambdaConfig::default())
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct CloudflareConfig {
    wrangler: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_name: Option<String>,
    /// This value we always get from the command line.
    #[serde(skip)]
    api_token: Option<String>,
}

impl CloudflareConfig {
    pub fn load_wrangler(&self, fs: &FileSystem) -> Result<Wrangler> {
        fs.load_file(WranglerFile)
    }

    pub fn wrangler_enabled(&self) -> bool {
        self.wrangler
    }

    fn load_platform(&self, args: &RunSubcommand) -> Result<BoxedPlatform> {
        let fs = FileSystem::new()?;
        // First, let's check and make sure we have an API token.
        let api_token = args.cloudflare_api_token().map(ToString::to_string).ok_or_else(|| miette!("No Cloudflare API token was provided. Either set the environment variable CLOUDFLARE_API_TOKEN, or provide it as a CLI flag."))?;
        // Now, load in the Wrangler file and fallback to its values, if enabled.
        let mut wranger_worker_name = None;
        let mut wranger_account_id = None;
        if self.wrangler_enabled() {
            let wrangler = self.load_wrangler(&fs)?;
            wranger_worker_name = Some(wrangler.worker().to_owned());
            wranger_account_id = wrangler.account_id().map(ToString::to_string);
        }

        // Next, get the name of the worker and the account id.
        let worker_name = args.cloudflare_worker_name().map(ToString::to_string).or_else(|| self.worker_name.clone())
            .or(wranger_worker_name)
            .ok_or_else(
                || miette!("No Cloudflare worker name provided. You must provide the name of a Cloudflare worker to deploy to.")
            )?;
        let account_id = args.cloudflare_account_id().map(ToString::to_string).or_else(|| self.account_id.clone())
        .or(wranger_account_id)
        .ok_or_else(|| miette!("No Cloudflare account id provided. You must provide the account id to deploy into, either via an environment variable, a CLI flag, or in your MultiTool.toml file or Wrangler.toml file."
        ))?;

        let client = CloudflareClient::new(&api_token);
        Ok(Box::new(CloudflareDeployment::new(
            client,
            account_id,
            worker_name,
        )))
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
pub struct AwsLambdaConfig {
    name: String,
    region: String,
}

impl AwsLambdaConfig {
    fn load_platform(&self) -> Result<BoxedPlatform> {
        todo!();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AwsApiGatewayConfig, CloudflareConfig, ConfigSection, IngressConfig, Manifest,
        MonitorConfig, PlatformConfig,
    };

    #[test]
    fn test_config_section_with_cloudflare() {
        let config = r#"
            cloudflare = { wrangler = true }
        "#;

        let config: ConfigSection = toml::from_str(config).unwrap();
        assert!(config.cloudflare.is_some());
        assert!(config.cloudflare.unwrap().wrangler);
    }

    /// A `config` field is required in every manifest.
    #[test]
    fn parse_example_no_config() {
        const RAW_MANIFEST: &str = r#"workspace = "wack"
application = "multitool"
"#;
        let observed = toml::from_str::<Manifest>(RAW_MANIFEST).is_err();
        assert!(observed);
    }

    #[test]
    fn parse_config_example1() {
        const RAW_MANIFEST: &str = r#"workspace = "wack"
application = "multitool"
config.monitor.aws-cloudwatch = {}

[config.ingress.aws-api-gateway]
stage-name = "foo"
resource-path = "bar"
resource-method = "baz"
gateway-name = "pop"
region = "us-east-2"

[config.platform.aws-lambda]
name = "buzz"
region = "us-east-2"
"#;
        let observed: Manifest = toml::from_str(RAW_MANIFEST).expect("manifest not parsable");

        assert_eq!(observed.workspace, Some("wack".to_string()));
        assert_eq!(observed.application, Some("multitool".to_string()));

        // Check monitor config
        matches!(
            observed
                .config
                .monitor
                .expect("Monitor config should be present"),
            MonitorConfig::AwsCloudwatch(_)
        );

        // Check ingress config
        if let Some(IngressConfig::AwsApiGateway(api_gateway)) = observed.config.ingress {
            assert_eq!(api_gateway.stage_name, "foo");
            assert_eq!(api_gateway.resource_path, "bar");
            assert_eq!(api_gateway.resource_method, "baz");
            assert_eq!(api_gateway.gateway_name, "pop");
            assert_eq!(api_gateway.region, "us-east-2");
        } else {
            panic!("Expected AwsApiGateway variant");
        }

        // Check platform config
        if let Some(PlatformConfig::AwsLambda(lambda)) = observed.config.platform {
            assert_eq!(lambda.name, "buzz");
            assert_eq!(lambda.region, "us-east-2");
        } else {
            panic!("Expected AwsLambda variant");
        }
    }
}
