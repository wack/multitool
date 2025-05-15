use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::fs::{
    FileSystem,
    wrangler::{Wrangler, WranglerFile},
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
    config: Option<ConfigSection>,
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
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct CloudflareConfig {
    wrangler: bool,
}

impl CloudflareConfig {
    pub fn load_wrangler(&self, fs: &FileSystem) -> Result<Wrangler> {
        fs.load_file(WranglerFile)
    }
}
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct ConfigSection {
    #[serde(default)]
    monitor: MonitorConfig,
    #[serde(default)]
    ingress: IngressConfig,
    #[serde(default)]
    platform: PlatformConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cloudflare: Option<CloudflareConfig>,
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

impl Default for PlatformConfig {
    fn default() -> Self {
        Self::AwsLambda(AwsLambdaConfig::default())
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
pub struct AwsLambdaConfig {
    name: String,
    region: String,
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
        assert!(config.cloudflare.unwrap().wrangler);
    }

    #[test]
    fn parse_example_no_config() {
        const RAW_MANIFEST: &str = r#"workspace = "wack"
application = "multitool"
"#;
        let observed: Manifest = toml::from_str(RAW_MANIFEST).expect("manifest not parsable");

        assert_eq!(observed.workspace, Some("wack".to_string()));
        assert_eq!(observed.application, Some("multitool".to_string()));
        assert!(observed.config.is_none());
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

        let config = observed.config.expect("Config should be present");

        // Check monitor config
        matches!(config.monitor, MonitorConfig::AwsCloudwatch(_));

        // Check ingress config
        if let IngressConfig::AwsApiGateway(api_gateway) = config.ingress {
            assert_eq!(api_gateway.stage_name, "foo");
            assert_eq!(api_gateway.resource_path, "bar");
            assert_eq!(api_gateway.resource_method, "baz");
            assert_eq!(api_gateway.gateway_name, "pop");
            assert_eq!(api_gateway.region, "us-east-2");
        } else {
            panic!("Expected AwsApiGateway variant");
        }

        // Check platform config
        if let PlatformConfig::AwsLambda(lambda) = config.platform {
            assert_eq!(lambda.name, "buzz");
            assert_eq!(lambda.region, "us-east-2");
        } else {
            panic!("Expected AwsLambda variant");
        }
    }
}
