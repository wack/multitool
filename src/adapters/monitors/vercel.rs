#[cfg(feature = "vercel")]
use async_trait::async_trait;
#[cfg(feature = "vercel")]
use chrono::{DateTime, Utc};
#[cfg(feature = "vercel")]
use derive_getters::Getters;
#[cfg(feature = "vercel")]
use tracing::info;

#[cfg(feature = "vercel")]
use crate::{
    Shutdownable,
    adapters::{backend::MonitorConfig, vercel::VercelClient as Client},
    metrics::ResponseStatusCode,
    stats::{CategoricalObservation, Group},
    subsystems::ShutdownResult,
};
#[cfg(feature = "vercel")]
use miette::Result;

#[cfg(feature = "vercel")]
use super::Monitor;

#[cfg(feature = "vercel")]
#[derive(Getters)]
pub struct VercelMonitor {
    client: Client,
    // The deployment ID of the baseline version
    baseline_deployment_id: Option<String>,
    // The deployment ID of the canary version
    canary_deployment_id: Option<String>,
    // The time we started querying
    _start_time: DateTime<Utc>,
}

#[cfg(feature = "vercel")]
impl VercelMonitor {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            baseline_deployment_id: None,
            canary_deployment_id: None,
            _start_time: Utc::now(),
        }
    }
}

#[cfg(feature = "vercel")]
#[async_trait]
impl Monitor for VercelMonitor {
    type Item = CategoricalObservation<5, ResponseStatusCode>;

    fn get_config(&self) -> MonitorConfig {
        MonitorConfig::Vercel {
            api_token: self.client.api_token().clone(),
            project_name: self.client.project_name().clone(),
            team_id: self.client.team_id().clone(),
        }
    }

    async fn query(&mut self) -> Result<Vec<Self::Item>> {
        info!("Querying Vercel for metrics (placeholder implementation)");

        // Note: Vercel's analytics API requires a paid plan
        // This is a placeholder implementation that returns empty metrics
        // In a real implementation, you would:
        // 1. Query Vercel's analytics API for each deployment
        // 2. Parse the response status codes
        // 3. Create CategoricalObservations for baseline and canary

        let metrics = Vec::new();

        // TODO: Implement actual Vercel analytics API integration
        // This would require calling Vercel's analytics endpoints
        // and parsing the response data

        Ok(metrics)
    }

    async fn set_canary_version_id(&mut self, canary_version_id: String) -> Result<()> {
        self.canary_deployment_id = Some(canary_version_id);
        Ok(())
    }

    async fn set_baseline_version_id(&mut self, baseline_version_id: String) -> Result<()> {
        self.baseline_deployment_id = Some(baseline_version_id);
        Ok(())
    }
}

#[cfg(feature = "vercel")]
#[async_trait]
impl Shutdownable for VercelMonitor {
    async fn shutdown(&mut self) -> ShutdownResult {
        // When we get the shutdown signal, we stop querying
        Ok(())
    }
}
