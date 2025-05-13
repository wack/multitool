use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::fs::FileSystem;

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
    cloudflare_worker: Option<CloudflareWorker>,
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct ConfigSection {
    #[serde(default)]
    monitor: MonitorConfig,
    #[serde(default)]
    ingress: IngressConfig,
    #[serde(default)]
    platform: PlatformConfig,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "kebab-case")]
#[serde(untagged)]
pub enum MonitorConfig {
    // Struct variants
    AwsCloudwatch(AwsCloudwatch),
    CloudflareWorker(CloudflareWorker),
    #[serde(rename = "cloudflare-worker")]
    CloudflareWorkerReference,
}

impl MonitorConfig {
    pub fn cloudflare_account_id<'a>(
        &'a self,
        reference: Option<&'a CloudflareWorker>,
    ) -> Option<&'a str> {
        match self {
            Self::CloudflareWorker(worker) => Some(&worker.account_id),
            Self::CloudflareWorkerReference => reference.map(|r| r.account_id.as_ref()),
            _ => None,
        }
    }

    pub fn cloudflare_worker_name<'a>(
        &'a self,
        reference: Option<&'a CloudflareWorker>,
    ) -> Option<&'a str> {
        match self {
            Self::CloudflareWorker(worker) => Some(&worker.worker_name),
            Self::CloudflareWorkerReference => reference.map(|r| r.worker_name.as_ref()),
            _ => None,
        }
    }
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self::AwsCloudwatch(AwsCloudwatch::default())
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct AwsCloudwatch {}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Debug)]
pub struct CloudflareWorker {
    worker_name: String,
    account_id: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Debug)]
#[serde(rename_all = "kebab-case")]
#[serde(untagged)]
pub enum IngressConfig {
    AwsApiGateway(AwsApiGateway),
    CloudflareWorker(CloudflareWorker),
    #[serde(rename = "cloudflare-worker")]
    CloudflareWorkerReference,
}

impl Default for IngressConfig {
    fn default() -> Self {
        Self::AwsApiGateway(AwsApiGateway::default())
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct AwsApiGateway {
    stage_name: String,
    gateway_name: String,
    resource_path: String,
    resource_method: String,
    region: String,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
#[serde(untagged)]
pub enum PlatformConfig {
    AwsLambda(AwsLambda),
    #[serde(rename = "cloudflare-worker")]
    CloudflareWorkerReference,
}

impl Default for PlatformConfig {
    fn default() -> Self {
        Self::AwsLambda(AwsLambda::default())
    }
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
pub struct AwsLambda {
    name: String,
    region: String,
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

#[cfg(test)]
mod tests {
    use super::{IngressConfig, Manifest, MonitorConfig, PlatformConfig};

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

#[test]
fn parse_config_cloudflare_worker() {
    const RAW_MANIFEST: &str = r#"workspace = "wack"
application = "multitool"

[config.monitor.cloudflare-worker]
worker-name = "my-worker"
account-id = "abc123def456"

[config.ingress.aws-api-gateway]
stage-name = "foo"
resource-path = "bar"
resource-method = "baz"
gateway-name = "pop"
region = "us-east-2"

[config.platform.aws-lambda]
name = "buzz"
region = "us-east-2"

[cloudflare-worker]
worker-name = "top-level-worker"
account-id = "xyz789"
"#;
    let observed: Manifest = toml::from_str(RAW_MANIFEST).expect("manifest not parsable");

    assert_eq!(observed.workspace, Some("wack".to_string()));
    assert_eq!(observed.application, Some("multitool".to_string()));

    let config = observed.config.expect("Config should be present");

    // Check monitor config
    if let MonitorConfig::CloudflareWorker(worker) = config.monitor {
        assert_eq!(worker.worker_name, "my-worker");
        assert_eq!(worker.account_id, "abc123def456");
    } else {
        panic!("Expected CloudflareWorker variant");
    }

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

    // Check top-level cloudflare worker config
    let cloudflare = observed
        .cloudflare_worker
        .expect("Cloudflare worker config should be present");
    assert_eq!(cloudflare.worker_name, "top-level-worker");
    assert_eq!(cloudflare.account_id, "xyz789");
}

#[test]
fn parse_monitor_string_variants() {
    // Test AWS string variant
    const AWS_MANIFEST: &str = r#"
config.monitor = "aws"
"#;
    let aws_config: Manifest = toml::from_str(AWS_MANIFEST).expect("manifest not parsable");
    assert!(matches!(
        aws_config.config.unwrap().monitor,
        MonitorConfig::Aws
    ));

    // Test Cloudflare Worker string variant
    const CLOUDFLARE_MANIFEST: &str = r#"
config.monitor = "cloudflare-worker"
"#;
    let cloudflare_config: Manifest =
        toml::from_str(CLOUDFLARE_MANIFEST).expect("manifest not parsable");
    assert!(matches!(
        cloudflare_config.config.unwrap().monitor,
        MonitorConfig::CloudflareWorkerReference
    ));
}

#[test]
fn parse_ingress_and_platform_references() {
    const MANIFEST: &str = r#"
[config]
ingress = "cloudflare-worker"
platform = "cloudflare-worker"
"#;
    let config: Manifest = toml::from_str(MANIFEST).expect("manifest not parsable");
    let config_section = config.config.expect("Config should be present");

    assert!(matches!(
        config_section.ingress,
        IngressConfig::CloudflareWorkerReference
    ));

    assert!(matches!(
        config_section.platform,
        PlatformConfig::CloudflareWorkerReference
    ));
}

#[test]
fn test_cloudflare_account_id() {
    // Test CloudflareWorker variant
    let worker = CloudflareWorker {
        worker_name: "test-worker".to_string(),
        account_id: "acc123".to_string(),
    };
    let config = MonitorConfig::CloudflareWorker(worker);
    assert_eq!(config.cloudflare_account_id(None), Some("acc123"));

    // Test CloudflareWorkerReference variant with reference
    let reference = CloudflareWorker {
        worker_name: "ref-worker".to_string(),
        account_id: "ref456".to_string(),
    };
    let config = MonitorConfig::CloudflareWorkerReference;
    assert_eq!(
        config.cloudflare_account_id(Some(&reference)),
        Some("ref456")
    );

    // Test CloudflareWorkerReference variant without reference
    assert_eq!(config.cloudflare_account_id(None), None);

    // Test other variants return None
    let config = MonitorConfig::AwsCloudwatch(AwsCloudwatch::default());
    assert_eq!(config.cloudflare_account_id(None), None);
    let config = MonitorConfig::Aws;
    assert_eq!(config.cloudflare_account_id(None), None);
}
