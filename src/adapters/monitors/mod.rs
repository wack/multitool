use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use miette::Result;

use crate::{
    Shutdownable,
    metrics::ResponseStatusCode,
    stats::{CategoricalObservation, Observation},
};

/// StatusCode is a type alias for the unwieldly named type on the right.
pub type StatusCode = CategoricalObservation<5, ResponseStatusCode>;

pub use cloudflare::CloudflareMonitor;
pub use cloudwatch::CloudWatch;

use super::backend::MonitorConfig;

// TODO: For now, we require all monitors to monitor just
// the status code. We may have trouble with the Builder in the
// future because we can't really genericize it. But when we add
// more metrics, we'll upgrade Monitors to handle them all, simultaniously,
// and there may not be a generic parameter on the Monitor type anymore.
pub type BoxedMonitor = Box<dyn Monitor<Item = StatusCode> + Send + Sync>;

#[async_trait]
pub trait Monitor: Shutdownable {
    type Item: Observation;

    /// Returns the configuration data for this monitor
    fn get_config(&self) -> MonitorConfig;
    async fn query(&mut self) -> Result<Vec<Self::Item>>;
    async fn set_canary_version_id(&mut self, canary_version_id: String) -> Result<()>;
    async fn set_baseline_version_id(&mut self, baseline_version_id: String) -> Result<()>;

    /// Print a warning message if we have low metrics, but only if it's been 3 minutes since we started
    fn check_metrics_count(
        &self,
        total_metrics_count: u32,
        start_time: chrono::DateTime<chrono::Utc>,
        start_query_time: chrono::DateTime<chrono::Utc>,
        end_query_time: chrono::DateTime<chrono::Utc>,
    ) {
        if ((Utc::now() - start_time) > TimeDelta::minutes(3)) && (total_metrics_count < 20) {
            // Sometimes the elapsed_time is 59s and not 1 full minute, so we want to have a floor of at least 1 min
            let elapsed_time = std::cmp::max(1, (end_query_time - start_query_time).num_minutes());
            let elapsed_time_str = if elapsed_time > 1 {
                format!("{elapsed_time} minutes")
            } else {
                format!("{elapsed_time} minute")
            };
            tracing::warn!(
                "Warning: MultiTool has collected {} metrics in the past {}. More traffic will produce more accurate results.",
                total_metrics_count,
                elapsed_time_str,
            );
        }
    }
}

mod cloudflare;
mod cloudwatch;
