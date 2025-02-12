use std::ops::Deref;

use openapi::{
    apis::configuration::Configuration, models::AwsIngressConfigOneOfRestApiGatewayConfig,
};

use crate::Cli;

pub struct BackendConfig {
    conf: Configuration,
}

impl From<&Cli> for BackendConfig {
    fn from(cli: &Cli) -> Self {
        Self::new(cli.origin())
    }
}

impl BackendConfig {
    pub fn new<T: AsRef<str>>(origin: Option<T>) -> Self {
        // • Convert the Option<T> to a String.
        let origin = origin.map(|val| val.as_ref().to_owned());
        // • Set up the default configuration values.
        let mut conf = Configuration {
            ..Configuration::default()
        };
        // • Override the default origin.
        if let Some(origin) = origin {
            conf.base_path = origin;
        }
        Self { conf }
    }
}

impl Deref for BackendConfig {
    type Target = Configuration;

    fn deref(&self) -> &Self::Target {
        &self.conf
    }
}

// TODO: We should pull these types out into a shared
// crate, open source, that we can can use on both the
// CLI and the backend.
// MultiToolCore. It can be within the CLI workspace, and we
// can pull it into the backend.

#[non_exhaustive]
pub enum PlatformConfig {
    Lambda(LambdaConfig),
}

// TODO: Add the derive Getter macro.
pub struct LambdaConfig {
    pub region: String,
    pub name: String,
}

#[non_exhaustive]
pub enum IngressConfig {
    RestApiGateway(AwsRestApiGatewayConfig),
}

pub struct AwsRestApiGatewayConfig {
    region: String,
    gateway_name: String,
    stage_name: String,
    resource_path: String,
    resource_method: String,
}

#[non_exhaustive]
pub enum MonitorConfig {
    AwsCloudwatchMetrics(CloudwatchMetricsConfig),
}

pub struct CloudwatchMetricsConfig {
    // TODO: We probably need the slug to
    //       the lambda name.
}
