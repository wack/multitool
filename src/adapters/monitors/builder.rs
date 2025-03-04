use super::cloudwatch::CloudWatch;
use async_trait::async_trait;
use multitool_sdk::models::{MonitorConfig, MonitorConfigOneOfAwsCloudwatchMetrics};

use super::BoxedMonitor;

#[async_trait]
trait Builder {
    async fn build(self) -> BoxedMonitor;
}

pub(crate) struct MonitorBuilder {
    config: MonitorConfig,
}

impl MonitorBuilder {
    pub(crate) fn new(config: MonitorConfig) -> Self {
        Self { config }
    }

    pub async fn build(self) -> BoxedMonitor {
        Builder::build(self).await
    }
}

#[async_trait]
impl Builder for MonitorBuilder {
    async fn build(self) -> BoxedMonitor {
        match self.config {
            MonitorConfig::MonitorConfigOneOf(monitor_config) => {
                AwsCloudwatchMetricsMonitorBuilder::new(*monitor_config.aws_cloudwatch_metrics)
                    .build()
                    .await
            }
        }
    }
}

struct AwsCloudwatchMetricsMonitorBuilder {
    conf: MonitorConfigOneOfAwsCloudwatchMetrics,
}

impl AwsCloudwatchMetricsMonitorBuilder {
    fn new(conf: MonitorConfigOneOfAwsCloudwatchMetrics) -> Self {
        Self { conf }
    }
}

#[async_trait]
impl Builder for AwsCloudwatchMetricsMonitorBuilder {
    async fn build(self) -> BoxedMonitor {
        // TODO: Plumb the values in correctly.
        // let gateway_name = self.conf.gateway_name;
        // let stage_name = self.conf.stage_name;
        let gateway_name = "".to_owned();
        let stage_name = "".to_owned();
        let region = self.conf.region;
        let cloudwatch_monitor = CloudWatch::builder()
            .gateway_name(gateway_name)
            .stage_name(stage_name)
            .region(region)
            .build()
            .await;
        Box::new(cloudwatch_monitor)
    }
}

#[cfg(test)]
mod tests {
    use miette::{IntoDiagnostic, Result};
    use multitool_sdk::models::MonitorConfig;
    use serde_json::{Value, json};

    use super::MonitorBuilder;
    use crate::adapters::BoxedMonitor;

    // TODO: I think we're going to need a LogGroup here.
    fn monitor_json() -> Value {
        json!({
            "aws_cloudwatch_metrics": {
                "region": "us-east-2"
            }
        })
    }

    #[tokio::test]
    async fn parse_monitor_config() -> Result<()> {
        let config_json = serde_json::to_string(&monitor_json()).into_diagnostic()?;
        let config_object: MonitorConfig = serde_json::from_str(&config_json).into_diagnostic()?;
        let _: BoxedMonitor = MonitorBuilder::new(config_object).build().await;
        Ok(())
    }
}
