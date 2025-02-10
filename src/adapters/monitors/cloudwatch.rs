use async_trait::async_trait;

use crate::{
    metrics::ResponseStatusCode,
    stats::{CategoricalObservation, Group},
    utils::load_default_aws_config,
};
use aws_sdk_cloudwatch::{
    client::Client as AwsClient,
    types::{Dimension, Metric, MetricDataQuery, MetricStat},
};
use aws_smithy_types::DateTime as AwsDateTime;
use chrono::{DateTime, Duration, Utc};

use super::Monitor;

pub struct CloudWatch {
    client: AwsClient,
    gateway_name: String,
    stage_name: String,
}

impl CloudWatch {
    pub async fn new(gateway_name: &str, stage_name: &str) -> Self {
        let config = load_default_aws_config().await;
        let client = aws_sdk_cloudwatch::Client::new(config);
        Self {
            client,
            gateway_name: gateway_name.to_owned(),
            stage_name: stage_name.to_owned(),
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
    ) -> u32 {
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
                                    .name("Name")
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
                return response
                    .metric_data_results()
                    .iter()
                    .flat_map(|result| result.values())
                    .sum::<f64>() as u32;
            }
            Err(err) => {
                println!("Error querying cloudwatch metrics: {:?}", err);
                0
            }
        }
    }
}

#[async_trait]
impl Monitor for CloudWatch {
    type Item = CategoricalObservation<5, ResponseStatusCode>;

    async fn query(&mut self) -> Vec<Self::Item> {
        // This function queries the metrics that we care most about (2xx, 4xx, and 5xx errors),
        // compiles them into a list, then generates the correct number of
        // CategoricalObservations for each response code
        let now: DateTime<Utc> = Utc::now();
        let five_mins_ago: DateTime<Utc> = now - Duration::minutes(5);

        let control_count_future = self.query_cloudwatch(
            ApiMetric::Count,
            &self.gateway_name,
            &self.stage_name,
            Group::Control,
            now,
            five_mins_ago,
        );

        let control_4xx_future = self.query_cloudwatch(
            ApiMetric::Error4XX,
            &self.gateway_name,
            &self.stage_name,
            Group::Control,
            now,
            five_mins_ago,
        );

        let control_5xx_future = self.query_cloudwatch(
            ApiMetric::Error5XX,
            &self.gateway_name,
            &self.stage_name,
            Group::Control,
            now,
            five_mins_ago,
        );

        let canary_count_future = self.query_cloudwatch(
            ApiMetric::Count,
            &self.gateway_name,
            &self.stage_name,
            Group::Experimental,
            now,
            five_mins_ago,
        );

        let canary_4xx_future = self.query_cloudwatch(
            ApiMetric::Error4XX,
            &self.gateway_name,
            &self.stage_name,
            Group::Experimental,
            now,
            five_mins_ago,
        );

        let canary_5xx_future = self.query_cloudwatch(
            ApiMetric::Error5XX,
            &self.gateway_name,
            &self.stage_name,
            Group::Experimental,
            now,
            five_mins_ago,
        );

        let (control_count, control_4xx, control_5xx, canary_count, canary_4xx, canary_5xx) = tokio::join!(
            control_count_future,
            control_4xx_future,
            control_5xx_future,
            canary_count_future,
            canary_4xx_future,
            canary_5xx_future
        );

        let mut observations = Vec::new();

        // Collate all of our control metrics
        let control_2xx = (control_4xx + control_5xx) - control_count;

        // Since we need a CategoricalObservation for each instance of a response code
        // but AWS only returns us a total count, we need to make our own
        // list of observations, 1 per counted item
        observations.extend(
            std::iter::repeat(CategoricalObservation {
                group: Group::Control,
                outcome: ResponseStatusCode::_2XX,
            })
            .take(control_2xx as usize),
        );

        observations.extend(
            std::iter::repeat(CategoricalObservation {
                group: Group::Control,
                outcome: ResponseStatusCode::_4XX,
            })
            .take(control_4xx as usize),
        );

        observations.extend(
            std::iter::repeat(CategoricalObservation {
                group: Group::Control,
                outcome: ResponseStatusCode::_5XX,
            })
            .take(control_5xx as usize),
        );

        // Collate all of our canary/experimental metrics
        let canary_2xx = (canary_4xx + canary_5xx) - canary_count;

        observations.extend(
            std::iter::repeat(CategoricalObservation {
                group: Group::Experimental,
                outcome: ResponseStatusCode::_2XX,
            })
            .take(canary_2xx as usize),
        );

        observations.extend(
            std::iter::repeat(CategoricalObservation {
                group: Group::Experimental,
                outcome: ResponseStatusCode::_4XX,
            })
            .take(canary_4xx as usize),
        );

        observations.extend(
            std::iter::repeat(CategoricalObservation {
                group: Group::Experimental,
                outcome: ResponseStatusCode::_5XX,
            })
            .take(canary_5xx as usize),
        );

        return observations;
    }
}
