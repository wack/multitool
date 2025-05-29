use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use derive_getters::Getters;
use tracing::{debug, info, trace};

use crate::{
    Shutdownable,
    adapters::{CloudflareClient as Client, backend::MonitorConfig},
    metrics::ResponseStatusCode,
    stats::{CategoricalObservation, Group},
    subsystems::ShutdownResult,
};
use miette::Result;

use super::Monitor;

#[derive(Getters)]
pub struct CloudflareMonitor {
    client: Client,
    // The version id of the baseline version
    control_version_id: Option<String>,
    // The version id of the canary version
    canary_version_id: Option<String>,
    // The time we started querying
    start_time: DateTime<Utc>,
    // The time we last queried
    last_query_time: DateTime<Utc>,
}

impl CloudflareMonitor {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            control_version_id: None,
            canary_version_id: None,
            start_time: Utc::now(),
            last_query_time: Utc::now() - Duration::minutes(5),
        }
    }
}

#[async_trait]
impl Monitor for CloudflareMonitor {
    type Item = CategoricalObservation<5, ResponseStatusCode>;

    fn get_config(&self) -> MonitorConfig {
        MonitorConfig::CloudflareWorkersObservability {
            account_id: self.client.account_id().clone(),
            worker_name: self.client.worker_name().clone(),
        }
    }

    async fn query(&mut self) -> Result<Vec<Self::Item>> {
        info!("Querying Cloudflare for new metrics.");

        // This function queries the metrics that we care most about (2xx, 4xx, and 5xx errors),
        // compiles them into a list, then generates the correct number of
        // CategoricalObservations for each response code
        let utc_now = Utc::now();
        let end_query_time: DateTime<Utc> = Utc::now();
        let start_query_time = self.last_query_time;

        let mut metrics = Vec::new();

        trace!("Control version id: {:?}", self.control_version_id);
        // Query all control metrics, but only if we've already received a control version id
        if let Some(control_version_id) = &self.control_version_id {
            let control_2xx_future = self.client.collect_metrics(
                control_version_id.clone(),
                200,
                299,
                start_query_time,
                end_query_time,
            );

            let control_4xx_future = self.client.collect_metrics(
                control_version_id.clone(),
                400,
                499,
                start_query_time,
                end_query_time,
            );

            let control_5xx_future = self.client.collect_metrics(
                control_version_id.clone(),
                500,
                599,
                start_query_time,
                end_query_time,
            );

            let (control_2xx_result, control_4xx_result, control_5xx_result) =
                tokio::join!(control_2xx_future, control_4xx_future, control_5xx_future,);

            let control_4xx = control_4xx_result?;
            let control_5xx = control_5xx_result?;
            let control_2xx = control_2xx_result?;

            debug!("Control: 2xx: {control_2xx}, 4xx: {control_4xx}, 5xx: {control_5xx}");

            let mut baseline = CategoricalObservation::new(Group::Control, utc_now);
            baseline.increment_by(&ResponseStatusCode::_2XX, control_2xx);
            baseline.increment_by(&ResponseStatusCode::_4XX, control_4xx);
            baseline.increment_by(&ResponseStatusCode::_5XX, control_5xx);

            metrics.push(baseline);
        }

        trace!("Canary version id: {:?}", self.canary_version_id);
        // Query all canary metrics, but only if we've already received a control version id
        if let Some(canary_version_id) = &self.canary_version_id {
            let canary_2xx_future = self.client.collect_metrics(
                canary_version_id.clone(),
                200,
                299,
                start_query_time,
                end_query_time,
            );

            let canary_4xx_future = self.client.collect_metrics(
                canary_version_id.clone(),
                400,
                499,
                start_query_time,
                end_query_time,
            );

            let canary_5xx_future = self.client.collect_metrics(
                canary_version_id.clone(),
                500,
                599,
                start_query_time,
                end_query_time,
            );

            let (canary_2xx_result, canary_4xx_result, canary_5xx_result) =
                tokio::join!(canary_2xx_future, canary_4xx_future, canary_5xx_future);

            let canary_4xx = canary_4xx_result?;
            let canary_5xx = canary_5xx_result?;
            let canary_2xx = canary_2xx_result?;

            debug!("Canary: 2xx: {canary_2xx}, 4xx: {canary_4xx}, 5xx: {canary_5xx}");

            let mut canary = CategoricalObservation::new(Group::Experimental, utc_now);
            canary.increment_by(&ResponseStatusCode::_2XX, canary_2xx);
            canary.increment_by(&ResponseStatusCode::_4XX, canary_4xx);
            canary.increment_by(&ResponseStatusCode::_5XX, canary_5xx);

            metrics.push(canary);
        }

        // Update the timer to skip old values. This has to occur
        // before the ? in the next block, or else we might
        // never advance our timer.
        self.last_query_time = end_query_time;

        let total_metrics_count = metrics.iter().map(|m| m.histogram().total()).sum();
        self.check_metrics_count(
            total_metrics_count,
            self.start_time,
            start_query_time,
            end_query_time,
        );

        Ok(metrics)
    }

    async fn set_canary_version_id(&mut self, canary_version_id: String) -> Result<()> {
        self.canary_version_id = Some(canary_version_id);
        Ok(())
    }

    // TODO: standardize naming to either baseline or control
    async fn set_baseline_version_id(&mut self, baseline_version_id: String) -> Result<()> {
        self.control_version_id = Some(baseline_version_id);
        Ok(())
    }
}

#[async_trait]
impl Shutdownable for CloudflareMonitor {
    async fn shutdown(&mut self) -> ShutdownResult {
        // When we get the shutdown signal, all we need to do is not query anymore
        Ok(())
    }
}
