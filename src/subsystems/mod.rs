pub use action_listener::{ActionListenerSubsystem, ACTION_LISTENER_SUBSYSTEM_NAME};
pub use ingress::{IngressSubsystem, INGRESS_SUBSYSTEM_NAME};
pub use monitor::{MonitorSubsystem, MONITOR_SUBSYSTEM_NAME};
pub use platform::{PlatformSubsystem, PLATFORM_SUBSYSTEM_NAME};

use bon::Builder;

mod action_listener;
mod ingress;
mod monitor;
mod platform;

pub enum IngressConfig {
    Aws(AwsIngressConfigBuilder),
}

#[derive(Builder)]
pub struct AwsIngressConfig {
    region: String,
    ingress: AwsIngressBuilder,
}

pub enum AwsIngressBuilder {
    RestApiGateway(RestApiGatewayConfigBuilder),
}

#[derive(Builder)]
pub struct RestApiGatewayConfig {
    region: String,
    gateway_name: String,
    stage_name: String,
    resource_path: String,
    resource_method: String,
}

pub enum PlatformConfig {
    Aws(AwsPlatformConfigBuilder),
}

#[derive(Builder)]
pub struct AwsPlatformConfig {
    region: String,
    platform: AwsPlatformBuilder,
}

pub enum AwsPlatformBuilder {
    Lambda(LambdaPlatformConfigBuilder),
}

#[derive(Builder)]
pub struct LambdaPlatformConfig {
    region: String,
    name: String,
}

pub enum MonitorConfig {
    Aws(AwsMonitorConfigBuilder),
}

#[derive(Builder)]
pub struct AwsMonitorConfig {
    region: String,
    monitor: AwsMonitorBuilder,
}

pub enum AwsMonitorBuilder {
    CloudwatchMetrics(CloudwatchMetricsConfigBuilder),
}

#[derive(Builder)]
pub struct CloudwatchMetricsConfig {}

#[cfg(test)]
mod tests {
    use openapi::models::ApplicationConfig;

    #[test]
    fn parse_app_config() {
        // • Get the JSON describing this configuration.
        // • Marshal it into a type.
        // • Try to parse it into a domain type.
        let config = ApplicationConfig::default();
        // TODO: Turn this shit into traits.
        todo!()
    }
}
