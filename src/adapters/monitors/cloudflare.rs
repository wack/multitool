use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeDelta, Utc};
use tracing::{debug, info};

use crate::{
    Shutdownable,
    adapters::CloudFlareClient as Client,
    metrics::ResponseStatusCode,
    stats::{CategoricalObservation, Group},
    subsystems::ShutdownResult,
};
use miette::Result;

use super::Monitor;

pub struct CloudFlareMonitor {
    client: Client,
    // Cloudflare account id
    account_id: String,
    // Cloudflare worker name
    worker_name: String,
    // The version id of the baseline version
    control_version_id: String,
    // The version id of the canary version
    canary_version_id: String,
    // The time we started querying
    start_time: DateTime<Utc>,
    // The time we last queried
    last_query_time: DateTime<Utc>,
}

impl CloudFlareMonitor {
    pub fn new(
        client: Client,
        account_id: String,
        worker_name: String,
        control_version_id: String,
        canary_version_id: String,
    ) -> Self {
        Self {
            client,
            account_id,
            worker_name,
            control_version_id,
            canary_version_id,
            start_time: Utc::now(),
            last_query_time: Utc::now() - Duration::minutes(5),
        }
    }
}

#[async_trait]
impl Monitor for CloudFlareMonitor {
    type Item = CategoricalObservation<5, ResponseStatusCode>;

    async fn query(&mut self) -> Result<Vec<Self::Item>> {
        info!("Querying Cloudflare for new metrics.");
        // This function queries the metrics that we care most about (2xx, 4xx, and 5xx errors),
        // compiles them into a list, then generates the correct number of
        // CategoricalObservations for each response code
        let end_query_time: DateTime<Utc> = Utc::now();
        let start_query_time = self.last_query_time;

        let control_2xx_future = self.client.collect_metrics(
            self.account_id.clone(),
            self.worker_name.clone(),
            self.control_version_id.clone(),
            200,
            299,
            start_query_time,
            end_query_time,
        );

        let control_4xx_future = self.client.collect_metrics(
            self.account_id.clone(),
            self.worker_name.clone(),
            self.control_version_id.clone(),
            400,
            499,
            start_query_time,
            end_query_time,
        );

        let control_5xx_future = self.client.collect_metrics(
            self.account_id.clone(),
            self.worker_name.clone(),
            self.control_version_id.clone(),
            500,
            599,
            start_query_time,
            end_query_time,
        );

        let canary_2xx_future = self.client.collect_metrics(
            self.account_id.clone(),
            self.worker_name.clone(),
            self.canary_version_id.clone(),
            200,
            299,
            start_query_time,
            end_query_time,
        );

        let canary_4xx_future = self.client.collect_metrics(
            self.account_id.clone(),
            self.worker_name.clone(),
            self.canary_version_id.clone(),
            400,
            499,
            start_query_time,
            end_query_time,
        );

        let canary_5xx_future = self.client.collect_metrics(
            self.account_id.clone(),
            self.worker_name.clone(),
            self.canary_version_id.clone(),
            500,
            599,
            start_query_time,
            end_query_time,
        );

        let (
            control_2xx_result,
            control_4xx_result,
            control_5xx_result,
            canary_2xx_result,
            canary_4xx_result,
            canary_5xx_result,
        ) = tokio::join!(
            control_2xx_future,
            control_4xx_future,
            control_5xx_future,
            canary_2xx_future,
            canary_4xx_future,
            canary_5xx_future
        );

        // Update the timer to skip old values. This has to occur
        // before the ? in the next block, or else we might
        // never advance our timer.
        self.last_query_time = end_query_time;
        let control_4xx = control_4xx_result?;
        let control_5xx = control_5xx_result?;
        let control_2xx = control_2xx_result?;
        let canary_4xx = canary_4xx_result?;
        let canary_5xx = canary_5xx_result?;
        let canary_2xx = canary_2xx_result?;

        self.check_metrics_count(
            control_2xx + control_4xx + control_5xx,
            canary_2xx + canary_4xx + canary_5xx,
            self.start_time,
            start_query_time,
            end_query_time,
        );

        debug!("Control: 2xx: {control_2xx}, 4xx: {control_4xx}, 5xx: {control_5xx}");
        debug!("Canary: 2xx: {canary_2xx}, 4xx: {canary_4xx}, 5xx: {canary_5xx}");

        let utc_now = Utc::now();
        let mut baseline = CategoricalObservation::new(Group::Control, utc_now);
        let mut canary = CategoricalObservation::new(Group::Experimental, utc_now);

        baseline.increment_by(&ResponseStatusCode::_2XX, control_2xx);
        baseline.increment_by(&ResponseStatusCode::_4XX, control_4xx);
        baseline.increment_by(&ResponseStatusCode::_5XX, control_5xx);

        canary.increment_by(&ResponseStatusCode::_2XX, canary_2xx);
        canary.increment_by(&ResponseStatusCode::_4XX, canary_4xx);
        canary.increment_by(&ResponseStatusCode::_5XX, canary_5xx);

        Ok(vec![baseline, canary])
    }
}

#[async_trait]
impl Shutdownable for CloudFlareMonitor {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!();
    }
}
