#[cfg(feature = "vercel")]
use crate::{
    Shutdownable, WholePercent,
    adapters::{
        backend::IngressConfig,
        vercel::VercelClient as Client,
    },
    subsystems::ShutdownResult,
};

#[cfg(feature = "vercel")]
use super::Ingress;
#[cfg(feature = "vercel")]
use async_trait::async_trait;
#[cfg(feature = "vercel")]
use derive_getters::Getters;
#[cfg(feature = "vercel")]
use miette::Result;
#[cfg(feature = "vercel")]
use tracing::{debug, info};

#[cfg(feature = "vercel")]
#[derive(Getters)]
pub struct VercelIngress {
    client: Client,
    // The deployment ID of the baseline version
    baseline_deployment_id: Option<String>,
    // The deployment ID of the canary version
    canary_deployment_id: Option<String>,
}

#[cfg(feature = "vercel")]
impl VercelIngress {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            baseline_deployment_id: None,
            canary_deployment_id: None,
        }
    }
}

#[cfg(feature = "vercel")]
#[async_trait]
impl Ingress for VercelIngress {
    fn get_config(&self) -> IngressConfig {
        IngressConfig::Vercel {
            project_name: self.client.project_name().clone(),
            team_id: self.client.team_id().clone(),
        }
    }

    async fn release_canary(
        &mut self,
        baseline_deployment_id: String,
        canary_deployment_id: String,
    ) -> Result<()> {
        debug!("Releasing canary in Vercel!");

        // Save the deployment IDs
        self.baseline_deployment_id = Some(baseline_deployment_id.clone());
        self.canary_deployment_id = Some(canary_deployment_id.clone());

        // Note: Vercel doesn't have built-in canary deployment traffic splitting
        // This is a placeholder implementation
        // In a real implementation, you would:
        // 1. Use Vercel's Edge Config or similar feature for traffic splitting
        // 2. Configure a middleware to route traffic based on percentages
        // 3. Or use Vercel's deployment promotion API

        info!("Canary deployment created: {}", canary_deployment_id);
        info!("Baseline deployment: {}", baseline_deployment_id);

        Ok(())
    }

    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        info!("Setting Vercel canary traffic to {percent} (placeholder implementation)");

        // Note: Vercel doesn't natively support percentage-based traffic splitting
        // like CloudFlare Workers. This would require:
        // 1. Setting up Edge Config or Edge Middleware
        // 2. Implementing custom traffic routing logic
        // 3. Using Vercel's API to update the configuration

        // For now, this is a placeholder
        debug!(
            "Would route {}% traffic to canary: {:?}",
            percent.as_i32(),
            self.canary_deployment_id
        );

        Ok(())
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        info!("Rolling back canary in Vercel (placeholder implementation)");

        // In a real implementation, this would:
        // 1. Remove the canary deployment from production
        // 2. Ensure 100% traffic goes to baseline
        // 3. Update Edge Config or middleware settings

        self.canary_deployment_id = None;

        Ok(())
    }

    async fn promote_canary(&mut self) -> Result<()> {
        info!("Promoting canary in Vercel!");

        // In a real implementation, this would:
        // 1. Make the canary deployment the production deployment
        // 2. Update DNS/routing to point to the canary
        // 3. Use Vercel's API to promote the deployment

        self.canary_deployment_id = None;

        Ok(())
    }
}

#[cfg(feature = "vercel")]
#[async_trait]
impl Shutdownable for VercelIngress {
    async fn shutdown(&mut self) -> ShutdownResult {
        // If there's no canary deployment ID set, there's nothing to rollback
        if self.canary_deployment_id.is_none() {
            debug!("No canary deployment ID set, nothing to rollback.");
            return Ok(());
        }

        self.rollback_canary().await?;
        Ok(())
    }
}
