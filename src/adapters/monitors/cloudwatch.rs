use std::cmp::max;

use async_trait::async_trait;
use bon::bon;
use multitool_sdk::models::CloudWatchDimensions;
use tracing::{debug, info, warn};

use crate::{
    Shutdownable,
    metrics::ResponseStatusCode,
    stats::{CategoricalObservation, Group},
    subsystems::ShutdownResult,
    utils::load_default_aws_config,
};
use aws_sdk_cloudwatch::{
    client::Client as AwsClient,
    types::{Dimension, Metric, MetricDataQuery, MetricStat},
};
use aws_smithy_types::DateTime as AwsDateTime;
use chrono::{DateTime, Duration, TimeDelta, Utc};
use miette::Result;

use super::Monitor;

pub struct CloudWatch {
    client: AwsClient,
    dimensions: Vec<CloudWatchDimensions>,
    region: String,
    // The time we started querying CloudWatch
    start_time: DateTime<Utc>,
    // The time we last queried CloudWatch
    last_query_time: DateTime<Utc>,
}

#[bon]
impl CloudWatch {
    #[builder]
    pub async fn new(region: String, dimensions: Vec<CloudWatchDimensions>) -> Self {
        let config = load_default_aws_config().await;
        let client = aws_sdk_cloudwatch::Client::new(config);
        Self {
            client,
            region,
            dimensions,
            start_time: Utc::now(),
            last_query_time: Utc::now() - Duration::minutes(5),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ApiMetric {
    Count,
    Error4XX,
    Error5XX,
}

impl ApiMetric {
    // Returns the value as a valid AWS query id name:
    // lower case, alphanumeric, and doesn't start with a number
    pub fn to_id(&self) -> &'static str {
        match self {
            ApiMetric::Count => "count",
            ApiMetric::Error4XX => "error4xx",
            ApiMetric::Error5XX => "error5xx",
        }
    }

    // Returns the value as a value AWS metric name
    pub fn to_metric_name(&self) -> &'static str {
        match self {
            ApiMetric::Count => "Count",
            ApiMetric::Error4XX => "4XXError",
            ApiMetric::Error5XX => "5XXError",
        }
    }
}

impl CloudWatch {
    // The default name AWS currently uses for canary stages in APIGs
    const CANARY_STAGE_SUFFIX: &'static str = "/Canary";

    fn get_stage_name(stage_name: &str, group: Group) -> String {
        // Since AWS standardizes the naming of the canary stage, we can just hard-code them
        if group == Group::Experimental {
            stage_name.to_owned() + Self::CANARY_STAGE_SUFFIX
        } else {
            stage_name.to_string()
        }
    }

    async fn query_cloudwatch(
        &self,
        metric_name: ApiMetric,
        api_gateway_name: &str,
        stage_name: &str,
        group: Group,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<u32> {
        // Builds a query that:
        // 1. Queries a specific API Gateway by name and stage,
        // 2. for Count, 4xxErrors, or 5xxErrors values,
        // 3. As a sum
        // 4. Over a 60s period
        // 5. Over the given window (5 mins, by default)
        let query = MetricDataQuery::builder()
            .id(metric_name.to_id())
            .metric_stat(
                MetricStat::builder()
                    .metric(
                        Metric::builder()
                            .namespace("AWS/ApiGateway")
                            .metric_name(metric_name.to_metric_name())
                            .dimensions(
                                Dimension::builder()
                                    .name("ApiName")
                                    .value(api_gateway_name)
                                    .build(),
                            )
                            .dimensions(
                                Dimension::builder()
                                    .name("Stage")
                                    .value(Self::get_stage_name(stage_name, group))
                                    .build(),
                            )
                            .build(),
                    )
                    .period(60)
                    .stat("Sum")
                    .build(),
            )
            .build();

        // AWS has custom DateTime formats, so we need to do a conversion first
        let response = self
            .client
            .get_metric_data()
            // AWS has custom DateTime formats, so we need to do a conversion first
            .start_time(AwsDateTime::from_secs(start.timestamp()))
            .end_time(AwsDateTime::from_secs(end.timestamp()))
            .metric_data_queries(query)
            .send()
            .await;

        // If there's an error, just return 0 results
        match response {
            Ok(response) => {
                // We need to sum all values provided since we have a period of 60s and time window of 5 mins,
                // so, in the worst case we get 0 values and in the best case we get 5 values
                Ok(response
                    .metric_data_results()
                    .iter()
                    .flat_map(|result| result.values())
                    .sum::<f64>() as u32)
            }
            Err(err) => {
                println!(
                    "Error querying cloudwatch metrics for {:?} {:?}: {:?}",
                    metric_name, group, err
                );
                Ok(0)
            }
        }
    }

    /// Checks if the number of metrics collected is low (< 20 per given period) and warn the user
    fn check_metrics_count(
        control_count: u32,
        canary_count: u32,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
    ) {
        if (control_count + canary_count) < 20 {
            // Sometimes the elapsed_time is 59s and not 1 full minute, so we want to have a floor of at least 1 min
            let elapsed_time = max(1, (end_time - start_time).num_minutes());
            let elapsed_time_str = if elapsed_time > 1 {
                format!("{elapsed_time} minutes")
            } else {
                format!("{elapsed_time} minute")
            };
            warn!(
                "Warning: MultiTool has collected {} metrics in the past {}. More traffic will produce more accurate results.",
                control_count + canary_count,
                elapsed_time_str,
            );
        }
    }
}

#[async_trait]
impl Shutdownable for CloudWatch {
    async fn shutdown(&mut self) -> ShutdownResult {
        // When we get the shutdown signal, all we need to do is not query CloudWatch
        Ok(())
    }
}

#[async_trait]
impl Monitor for CloudWatch {
    type Item = CategoricalObservation<5, ResponseStatusCode>;
    async fn query(&mut self) -> Result<Vec<Self::Item>> {
        info!("Querying CloudWatch for new metrics.");
        // This function queries the metrics that we care most about (2xx, 4xx, and 5xx errors),
        // compiles them into a list, then generates the correct number of
        // CategoricalObservations for each response code
        let end_query_time: DateTime<Utc> = Utc::now();
        let start_query_time = self.last_query_time;

        let control_count_future = self.query_cloudwatch(
            ApiMetric::Count,
            self.dimensions[0].value.as_ref(),
            self.dimensions[1].value.as_ref(),
            Group::Control,
            start_query_time,
            end_query_time,
        );

        let control_4xx_future = self.query_cloudwatch(
            ApiMetric::Error4XX,
            self.dimensions[0].value.as_ref(),
            self.dimensions[1].value.as_ref(),
            Group::Control,
            start_query_time,
            end_query_time,
        );

        let control_5xx_future = self.query_cloudwatch(
            ApiMetric::Error5XX,
            self.dimensions[0].value.as_ref(),
            self.dimensions[1].value.as_ref(),
            Group::Control,
            start_query_time,
            end_query_time,
        );

        let canary_count_future = self.query_cloudwatch(
            ApiMetric::Count,
            self.dimensions[0].value.as_ref(),
            self.dimensions[1].value.as_ref(),
            Group::Experimental,
            start_query_time,
            end_query_time,
        );

        let canary_4xx_future = self.query_cloudwatch(
            ApiMetric::Error4XX,
            self.dimensions[0].value.as_ref(),
            self.dimensions[1].value.as_ref(),
            Group::Experimental,
            start_query_time,
            end_query_time,
        );

        let canary_5xx_future = self.query_cloudwatch(
            ApiMetric::Error5XX,
            self.dimensions[0].value.as_ref(),
            self.dimensions[1].value.as_ref(),
            Group::Experimental,
            start_query_time,
            end_query_time,
        );

        let (
            control_count_result,
            control_4xx_result,
            control_5xx_result,
            canary_count_result,
            canary_4xx_result,
            canary_5xx_result,
        ) = tokio::join!(
            control_count_future,
            control_4xx_future,
            control_5xx_future,
            canary_count_future,
            canary_4xx_future,
            canary_5xx_future
        );

        // Update the timer to skip old values. This has to occur
        // before the ? in the next block, or else we might
        // never advance our timer.
        self.last_query_time = end_query_time;
        let control_4xx = control_4xx_result?;
        let control_5xx = control_5xx_result?;
        let control_count = control_count_result?;
        let canary_4xx = canary_4xx_result?;
        let canary_5xx = canary_5xx_result?;
        let canary_count = canary_count_result?;

        // Collate all of our control metrics
        let control_2xx = control_count - (control_4xx + control_5xx);
        // Collate all of our canary/experimental metrics
        let canary_2xx = canary_count - (canary_4xx + canary_5xx);

        // Print a warning message if we have low metrics, but only if it's been 3 minutes since we started
        if (Utc::now() - self.start_time) > TimeDelta::minutes(3) {
            Self::check_metrics_count(
                control_count,
                canary_count,
                start_query_time,
                end_query_time,
            );
        }

        debug!("Control: 2xx: {control_2xx}, 4xx: {control_4xx}, 5xx: {control_5xx}");

        debug!("Canary: 2xx: {canary_2xx}, 4xx: {canary_4xx}, 5xx: {canary_5xx}");

        let mut baseline = CategoricalObservation::new(Group::Control);
        let mut canary = CategoricalObservation::new(Group::Experimental);

        baseline.increment_by(&ResponseStatusCode::_2XX, control_2xx);
        baseline.increment_by(&ResponseStatusCode::_4XX, control_4xx);
        baseline.increment_by(&ResponseStatusCode::_5XX, control_5xx);

        canary.increment_by(&ResponseStatusCode::_2XX, canary_2xx);
        canary.increment_by(&ResponseStatusCode::_4XX, canary_4xx);
        canary.increment_by(&ResponseStatusCode::_5XX, canary_5xx);

        Ok(vec![baseline, canary])
    }
}
