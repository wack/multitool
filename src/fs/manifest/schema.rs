use std::{path::PathBuf, sync::OnceLock};

use miette::{Diagnostic, miette};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::error;

use crate::{
    adapters::{
        AwsApiGateway, BoxedIngress, BoxedMonitor, BoxedPlatform, CloudWatch, CloudflareClient,
        CloudflareMonitor, CloudflareWorkerIngress, CloudflareWorkerPlatform, LambdaPlatform,
        backend,
    },
    artifacts::LambdaZip,
    config::RunSubcommand,
    fs::{
        FileSystem,
        wrangler::{Wrangler, WranglerFile},
    },
};
use miette::Result;

/// The application manifest only needs to be loaded once, so we
/// cache it as a global singleton.
static APPLICATION_MANIFEST: OnceLock<Manifest> = OnceLock::new();

/// Return the application's manifest file, or an empty manifest if none
/// is found. Use the globally available cached file.
pub fn application_manifest() -> &'static Manifest {
    APPLICATION_MANIFEST.get_or_init(Manifest::load_or_default)
}

#[derive(Error, Debug, Diagnostic)]
#[error(
    "When configuration for Cloudflare is present, no configuration for ingress, monitor, or platform can be provided."
)]
struct CloudflareMutuallyExclusiveConfig;

#[derive(Error, Debug, Diagnostic)]
#[error("No platform config found.")]
struct MissingPlatformConfig;

#[derive(Error, Debug, Diagnostic)]
#[error("No ingress config found.")]
struct MissingIngressConfig;

#[derive(Error, Debug, Diagnostic)]
#[error("No monitor config found.")]
struct MissingMonitorConfig;

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
    /// Attempts to read a config file for this application, and
    /// returns an empty manifest file if none is found.
    pub(crate) fn load_or_default() -> Self {
        FileSystem::new().map_or(Self::default(), |fs| {
            fs.application_manifest().unwrap_or_default()
        })
    }

    pub fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }

    pub fn set_workspace<T: AsRef<str>>(&mut self, value: T) {
        self.workspace = Some(value.as_ref().to_owned());
    }

    pub fn set_application<T: AsRef<str>>(&mut self, value: T) {
        self.application = Some(value.as_ref().to_owned());
    }

    pub fn application(&self) -> Option<&str> {
        self.application.as_deref()
    }

    pub fn set_cloudflare_config(&mut self, config: CloudflareConfig) {
        self.config.cloudflare = Some(config);
    }

    pub fn set_platform_config(&mut self, config: PlatformConfig) {
        self.config.platform = Some(config);
    }

    pub fn set_ingress_config(&mut self, config: IngressConfig) {
        self.config.ingress = Some(config);
    }

    pub fn set_monitor_config(&mut self, config: MonitorConfig) {
        self.config.monitor = Some(config);
    }

    pub(crate) async fn load_platform(&self, args: &RunSubcommand) -> Result<BoxedPlatform> {
        self.config.load_platform(args).await
    }

    pub(crate) async fn load_ingress(&self, args: &RunSubcommand) -> Result<BoxedIngress> {
        self.config.load_ingress(args).await
    }

    pub(crate) async fn load_monitor(
        &self,
        args: &RunSubcommand,
        ingress: &BoxedIngress,
    ) -> Result<BoxedMonitor> {
        self.config.load_monitor(args, ingress).await
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
    async fn load_platform(&self, args: &RunSubcommand) -> Result<BoxedPlatform> {
        // Having cloudflare configured is mutually exclusive with having
        // platform configured. Error if both are set.
        match (&self.cloudflare, &self.platform) {
            (Some(_), Some(_)) => Err(CloudflareMutuallyExclusiveConfig.into()),
            (None, None) => Err(MissingPlatformConfig.into()),
            (None, Some(platform)) => platform.load_platform(args).await,
            (Some(cloudflare), None) => cloudflare.load_platform(args),
        }
    }

    async fn load_ingress(&self, args: &RunSubcommand) -> Result<BoxedIngress> {
        // Having cloudflare configured is mutually exclusive with having
        // ingress configured. Error if both are set.
        match (&self.cloudflare, &self.ingress) {
            (Some(_), Some(_)) => Err(CloudflareMutuallyExclusiveConfig.into()),
            (None, None) => Err(MissingIngressConfig.into()),
            (None, Some(ingress)) => ingress.load_ingress(args).await,
            (Some(cloudflare), None) => cloudflare.load_ingress(args),
        }
    }

    async fn load_monitor(
        &self,
        args: &RunSubcommand,
        ingress: &BoxedIngress,
    ) -> Result<BoxedMonitor> {
        // Having cloudflare configured is mutually exclusive with having
        // ingress configured. Error if both are set.
        match (&self.cloudflare, &self.monitor) {
            (Some(_), Some(_)) => Err(CloudflareMutuallyExclusiveConfig.into()),
            (None, None) => Err(MissingMonitorConfig.into()),
            (None, Some(monitor)) => monitor.load_monitor(args, ingress).await,
            (Some(cloudflare), None) => cloudflare.load_monitor(args),
        }
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorConfig {
    AwsCloudwatch(AwsCloudwatch),
    CloudflareObservability(CloudflareConfig),
}

impl MonitorConfig {
    async fn load_monitor(
        &self,
        args: &RunSubcommand,
        ingress: &BoxedIngress,
    ) -> Result<BoxedMonitor> {
        match self {
            MonitorConfig::AwsCloudwatch(aws_cloudwatch) => {
                aws_cloudwatch.load_monitor(args, ingress).await
            }
            MonitorConfig::CloudflareObservability(cloudflare_observability) => {
                cloudflare_observability.load_monitor(args)
            }
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct AwsCloudwatch {}

impl AwsCloudwatch {
    fn load_gateway_name(&self, args: &RunSubcommand, ingress: &BoxedIngress) -> Result<String> {
        let gateway_name = match ingress.get_config() {
            backend::IngressConfig::AwsRestApiGateway {
                gateway_name,
                region: _,
                stage_name: _,
                resource_path: _,
                resource_method: _,
            } => gateway_name,
            _ => return Err(miette!("Ingress is not an AWS API Gateway.")),
        };

        let gateway_name = args
            .aws_gateway_name()
            .map(ToString::to_string)
            .unwrap_or_else(|| gateway_name);

        Ok(gateway_name)
    }

    fn load_stage_name(&self, args: &RunSubcommand, ingress: &BoxedIngress) -> Result<String> {
        let stage_name = match ingress.get_config() {
            backend::IngressConfig::AwsRestApiGateway {
                stage_name,
                region: _,
                gateway_name: _,
                resource_path: _,
                resource_method: _,
            } => stage_name,
            _ => return Err(miette!("Ingress is not an AWS API Gateway.")),
        };

        let stage_name = args
            .aws_stage_name()
            .map(ToString::to_string)
            .unwrap_or_else(|| stage_name);

        Ok(stage_name)
    }

    async fn load_monitor(
        &self,
        args: &RunSubcommand,
        ingress: &BoxedIngress,
    ) -> Result<BoxedMonitor> {
        let gateway_name = self.load_gateway_name(args, ingress)?;
        let stage_name = self.load_stage_name(args, ingress)?;

        let monitor = CloudWatch::builder()
            .gateway_name(gateway_name)
            .stage_name(stage_name)
            .build()
            .await;

        Ok(Box::new(monitor))
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum IngressConfig {
    AwsApiGateway(AwsApiGatewayConfig),
    CloudflareWorkers(CloudflareConfig),
}

impl IngressConfig {
    async fn load_ingress(&self, args: &RunSubcommand) -> Result<BoxedIngress> {
        match self {
            IngressConfig::AwsApiGateway(config) => config.load_ingress(args).await,
            IngressConfig::CloudflareWorkers(config) => config.load_ingress(args),
        }
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

impl AwsApiGatewayConfig {
    pub fn new(
        stage_name: String,
        gateway_name: String,
        resource_path: String,
        resource_method: String,
        region: String,
    ) -> Self {
        Self {
            stage_name,
            gateway_name,
            resource_path,
            resource_method,
            region,
        }
    }

    async fn load_ingress(&self, _: &RunSubcommand) -> Result<BoxedIngress> {
        let ingress = AwsApiGateway::builder()
            .gateway_name(self.gateway_name.clone())
            .region(self.region.clone())
            .stage_name(self.stage_name.clone())
            .resource_path(self.resource_path.clone())
            .resource_method(self.resource_method.clone())
            .build()
            .await;
        Ok(Box::new(ingress))
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum PlatformConfig {
    AwsLambda(AwsLambdaConfig),
    CloudflareWorkers(CloudflareConfig),
}

impl PlatformConfig {
    async fn load_platform(&self, args: &RunSubcommand) -> Result<BoxedPlatform> {
        match self {
            PlatformConfig::AwsLambda(config) => config.load_platform(args).await,
            PlatformConfig::CloudflareWorkers(config) => config.load_platform(args),
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct AwsLambdaConfig {
    name: String,
    region: String,
    artifact_path: String,
}

impl AwsLambdaConfig {
    pub fn new(name: String, region: String, artifact_path: String) -> Self {
        Self {
            name,
            region,
            artifact_path,
        }
    }

    async fn load_platform(&self, args: &RunSubcommand) -> Result<BoxedPlatform> {
        let region: String = args
            .aws_region()
            .map(ToString::to_string)
            .unwrap_or_else(|| self.region.clone());

        let artifact = LambdaZip::load(self.artifact_path.clone()).await?;

        let platform = LambdaPlatform::builder()
            .name(self.name.clone())
            .region(region)
            .artifact(artifact)
            .build()
            .await;

        Ok(Box::new(platform))
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct CloudflareConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    wrangler: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    main_module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifact_path: Option<String>,
    /// We always get this value from the command line.
    #[serde(skip)]
    api_token: Option<String>,
}

impl CloudflareConfig {
    pub fn new(
        wrangler: bool,
        main_module: String,
        account_id: String,
        worker_name: String,
        artifact_path: String,
    ) -> Self {
        Self {
            wrangler: Some(wrangler),
            main_module: Some(main_module),
            account_id: Some(account_id),
            worker_name: Some(worker_name),
            artifact_path: Some(artifact_path),
            api_token: None, // This is set via CLI/env, not stored in manifest
        }
    }

    pub fn load_wrangler(&self, fs: &FileSystem) -> Result<Wrangler> {
        fs.load_file(WranglerFile)
    }

    pub fn wrangler_enabled(&self) -> bool {
        self.wrangler.unwrap_or(false)
    }

    fn load_api_token(&self, args: &RunSubcommand) -> Result<String> {
        args
            .cloudflare_api_token()
            .map(ToString::to_string)
            .ok_or_else(|| miette!("No Cloudflare API token was provided. Either set the environment variable CLOUDFLARE_API_TOKEN, or provide it as the --cloudflare-api-token CLI flag."))
    }

    fn load_worker_name(&self, fs: &FileSystem, args: &RunSubcommand) -> Result<String> {
        let wranger_worker_name = if self.wrangler_enabled() {
            let wrangler = self.load_wrangler(&fs)?;
            Some(wrangler.name().to_owned())
        } else {
            None
        };
        let worker_name = args.cloudflare_worker_name().map(ToString::to_string).or_else(|| self.worker_name.clone())
            .or(wranger_worker_name)
            .ok_or_else(
                || miette!("No Cloudflare worker name provided. You must provide the name of a Cloudflare worker, either via an environment variable, a CLI flag, or in your MultiTool manifest file or Wrangler.toml file.")
            )?;
        Ok(worker_name)
    }

    fn load_main_module(&self, fs: &FileSystem, args: &RunSubcommand) -> Result<String> {
        let wranger_main_module = if self.wrangler_enabled() {
            let wrangler = self.load_wrangler(&fs)?;
            Some(wrangler.main().to_owned())
        } else {
            None
        };
        let worker_main_module = args.cloudflare_main_module().map(ToString::to_string).or_else(|| self.main_module.clone())
            .or(wranger_main_module)
            .ok_or_else(
                || miette!("No Cloudflare main module provided. You must provide a main module, either via an environment variable, a CLI flag, or in your MultiTool manifest file or Wrangler.toml file.")
            )?;
        Ok(worker_main_module)
    }

    fn load_account_id(&self, fs: &FileSystem, args: &RunSubcommand) -> Result<String> {
        let wranger_account_id = if self.wrangler_enabled() {
            let wrangler = self.load_wrangler(fs)?;
            wrangler.account_id().map(ToString::to_string)
        } else {
            None
        };
        let account_id = args.cloudflare_account_id().map(ToString::to_string).or_else(|| self.account_id.clone())
            .or(wranger_account_id)
            .ok_or_else(|| miette!("No Cloudflare account id provided. You must provide the account id to deploy into, either via an environment variable, a CLI flag, or in your MultiTool manfiest file or Wrangler.toml file."))?;
        Ok(account_id)
    }

    fn load_artifact_path(&self, fs: &FileSystem, args: &RunSubcommand) -> Result<PathBuf> {
        if let Some(path) = args.artifact_path() {
            return Ok(path.as_ref().to_path_buf());
        }

        if let Some(path) = &self.artifact_path {
            return Ok(PathBuf::from(path));
        }

        // Finally, default to current working directory
        let current_dir = match fs.application_dir() {
            Err(err) => Err(err),
            Ok(Some(path)) => Ok(path),
            Ok(None) => std::env::current_dir()
                .map_err(|e| miette!("Failed to get current directory: {}", e)),
        }?;

        Ok(current_dir)
    }

    fn load_ingress(&self, args: &RunSubcommand) -> Result<BoxedIngress> {
        let fs = FileSystem::new()?;
        // First, let's check and make sure we have an API token.
        let api_token = self.load_api_token(args)?;
        let account_id = self.load_account_id(&fs, args)?;
        let worker_name = self.load_worker_name(&fs, args)?;
        let client = CloudflareClient::new(account_id, &api_token);

        Ok(Box::new(CloudflareWorkerIngress::new(client, worker_name)))
    }

    fn load_monitor(&self, args: &RunSubcommand) -> Result<BoxedMonitor> {
        let fs = FileSystem::new()?;
        // First, let's check and make sure we have an API token.
        let api_token = self.load_api_token(args)?;
        let account_id = self.load_account_id(&fs, args)?;
        let worker_name = self.load_worker_name(&fs, args)?;
        let client = CloudflareClient::new(account_id, &api_token);

        Ok(Box::new(CloudflareMonitor::new(client, worker_name)))
    }

    fn load_platform(&self, args: &RunSubcommand) -> Result<BoxedPlatform> {
        let fs = FileSystem::new()?;
        // First, let's check and make sure we have an API token.
        let api_token = self.load_api_token(args)?;
        let account_id = self.load_account_id(&fs, args)?;
        let worker_name = self.load_worker_name(&fs, args)?;
        let main_module = self.load_main_module(&fs, args)?;
        let artifact_path = self.load_artifact_path(&fs, args)?;
        let client = CloudflareClient::new(account_id, &api_token);

        Ok(Box::new(CloudflareWorkerPlatform::new(
            client,
            worker_name,
            artifact_path,
            main_module,
        )))
    }
}

#[cfg(test)]
mod tests {
    use crate::manifest::CloudflareConfig;

    use super::{ConfigSection, IngressConfig, Manifest, MonitorConfig, PlatformConfig};

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
    fn parse_aws_config_example() {
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
artifact-path = "my_code.zip"
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
            assert_eq!(api_gateway.region, "us-east-2".to_string());
        } else {
            panic!("Expected AwsApiGateway variant");
        }

        // Check platform config
        if let Some(PlatformConfig::AwsLambda(lambda)) = observed.config.platform {
            assert_eq!(lambda.name, "buzz");
            assert_eq!(lambda.region, "us-east-2");
            assert_eq!(lambda.artifact_path, "my_code.zip");
        } else {
            panic!("Expected AwsLambda variant");
        }
    }

    #[test]
    fn parse_full_cloudflare_config_example() {
        const RAW_MANIFEST: &str = r#"workspace = "wack"
application = "multitool"

[config.monitor.cloudflare-observability]
worker-name = "my_worker"
account-id = "abc123"

[config.platform.cloudflare-workers]
worker-name = "my_worker"
account-id = "abc123"

[config.ingress.cloudflare-workers]
worker-name = "my_worker"
account-id = "abc123"
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
            MonitorConfig::CloudflareObservability(_)
        );

        // Check ingress config
        if let Some(IngressConfig::CloudflareWorkers(config)) = observed.config.ingress {
            assert_eq!(config.account_id, Some("abc123".to_string()));
            assert_eq!(config.worker_name, Some("my_worker".to_string()));
        } else {
            panic!("Expected CloudflareWorkers variant");
        }

        // Check platform config
        if let Some(PlatformConfig::CloudflareWorkers(config)) = observed.config.platform {
            assert_eq!(config.account_id, Some("abc123".to_string()));
            assert_eq!(config.worker_name, Some("my_worker".to_string()));
        } else {
            panic!("Expected CloudflareWorkers variant");
        }
    }

    #[test]
    fn parse_short_cloudflare_config_example() {
        const RAW_MANIFEST: &str = r#"workspace = "wack"
application = "multitool"

[config.cloudflare]
worker-name = "my_worker"
account-id = "abc123"
main-module = "index.js"
artifact-path = "src"
"#;
        let observed: Manifest = toml::from_str(RAW_MANIFEST).expect("manifest not parsable");

        assert_eq!(observed.workspace, Some("wack".to_string()));
        assert_eq!(observed.application, Some("multitool".to_string()));

        // Check monitor config
        matches!(
            observed
                .config
                .cloudflare
                .clone()
                .expect("Cloudflare config should be present"),
            CloudflareConfig {
                account_id: Some(_),
                worker_name: Some(_),
                main_module: Some(_),
                artifact_path: Some(_),
                wrangler: None,
                api_token: None,
            }
        );

        // Check if values were set correctly
        if let Some(cloudflare) = observed.config.cloudflare {
            assert_eq!(cloudflare.account_id, Some("abc123".to_string()));
            assert_eq!(cloudflare.worker_name, Some("my_worker".to_string()));
            assert_eq!(cloudflare.main_module, Some("index.js".to_string()));
            assert_eq!(cloudflare.artifact_path, Some("src".to_string()));
            assert!(!cloudflare.wrangler_enabled());
        } else {
            panic!("Expected CloudflareConfig variant");
        }
    }

    #[test]
    fn test_config_section_with_cloudflare() {
        let config = r#"
            cloudflare = { wrangler = true }
        "#;

        let config: ConfigSection = toml::from_str(config).unwrap();
        assert!(config.cloudflare.is_some());
        assert!(config.cloudflare.unwrap().wrangler.unwrap_or(false));
    }
}
