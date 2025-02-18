pub use action_listener::{ActionListenerSubsystem, ACTION_LISTENER_SUBSYSTEM_NAME};
use async_trait::async_trait;
pub use ingress::{IngressSubsystem, INGRESS_SUBSYSTEM_NAME};
pub use monitor::{MonitorSubsystem, MONITOR_SUBSYSTEM_NAME};
pub use platform::{PlatformSubsystem, PLATFORM_SUBSYSTEM_NAME};

mod action_listener;
mod handle;
mod ingress;
mod monitor;
mod platform;

/// A ShutdownError is an error that occurred when a subsystem
/// was shutdown, or an error that forced the subsystem to shutdown.
pub type ShutdownResult = miette::Result<()>;

#[async_trait]
pub trait Shutdownable {
    async fn shutdown(&mut self) -> ShutdownResult;
}

/*

impl AwsIngressBuilder {
    fn new(region: String, config: AwsIngressConfigOneOf) -> Self {
        Self { region, config }
    }
}



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
*/
