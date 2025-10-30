use async_trait::async_trait;
use miette::{Result, miette};
use multitool_sdk::models::{RolloutStateStatus, RolloutStateType, RolloutStatus};
use tokio::sync::mpsc;
use tokio::time::{Duration, sleep};
use tracing::trace;

use crate::WholePercent;
use crate::adapters::{BackendClient, BoxedIngress, BoxedPlatform, RolloutMetadata};

use super::DeploymentMode;

pub struct ForceMode {
    backend: BackendClient,
    ingress: BoxedIngress,
    platform: BoxedPlatform,
    meta: RolloutMetadata,
}

const POLL_INTERVAL: Duration = Duration::from_secs(5);

impl ForceMode {
    pub fn new(
        backend: BackendClient,
        ingress: BoxedIngress,
        platform: BoxedPlatform,
        meta: RolloutMetadata,
    ) -> Self {
        Self {
            backend,
            ingress,
            platform,
            meta,
        }
    }
}

#[async_trait]
/// ForceMode is a deployment mode that immediately promotes a canary deployment to production, bypassing any canary analysis
/// We don't boot any subsystems for simplicity and instead just use the backend, ingress, platform, and NO monitor directly.
/// First, we deploy the canary, then we set its traffic to 100%, promote it, and mark the rollout as promoted.
impl DeploymentMode for ForceMode {
    async fn dispatch(mut self: Box<Self>) -> Result<()> {
        // Step 1: Poll for DEPLOY_CANARY state
        // This should be created as soon as the rollout starts, but if the rollout is queued, we need to wait.
        trace!("Waiting for DEPLOY_CANARY state...");
        let deploy_state = loop {
            let states = self.backend.poll_for_state(&self.meta).await?;

            if let Some(state) = states.iter().find(|s| {
                s.state_type == RolloutStateType::DeployCanary
                    && s.status == RolloutStateStatus::Pending
            }) {
                trace!("Found DEPLOY_CANARY state");
                break state.clone();
            }

            trace!("DEPLOY_CANARY state not found yet, waiting...");
            sleep(POLL_INTERVAL).await;
        };

        // Lock the DEPLOY_CANARY state
        let (deploy_done_tx, _deploy_done_rx) = mpsc::channel(1);
        let locked_deploy_state = self
            .backend
            .lock_state(&self.meta, &deploy_state, deploy_done_tx)
            .await?;

        // Step 2: Deploy the platform
        let (baseline_version_id, canary_version_id) = self.platform.deploy().await?;

        // Release the canary to the ingress with 0% traffic initially
        self.ingress
            .release_canary(baseline_version_id, canary_version_id)
            .await?;

        // Mark DEPLOY_CANARY state as DONE
        self.backend
            .mark_state_completed(&self.meta, &locked_deploy_state)
            .await?;

        // Step 3: Poll for SET_CANARY_TRAFFIC state (to 100%)
        // Again, this should be created as soon as the DEPLOY_CANARY state is marked as complete,
        // but if it doesn't get created, it's safer to wait.
        trace!("Waiting for SET_CANARY_TRAFFIC state...");
        let traffic_state = loop {
            let states = self.backend.poll_for_state(&self.meta).await?;

            if let Some(state) = states.iter().find(|s| {
                s.state_type == RolloutStateType::SetCanaryTraffic
                    && s.status == RolloutStateStatus::Pending
            }) {
                trace!("Found SET_CANARY_TRAFFIC state");
                break state.clone();
            }

            trace!("SET_CANARY_TRAFFIC state not found yet, waiting...");
            sleep(POLL_INTERVAL).await;
        };

        // Lock the SET_CANARY_TRAFFIC state
        let (traffic_done_tx, _traffic_done_rx) = mpsc::channel(1);
        let locked_traffic_state = self
            .backend
            .lock_state(&self.meta, &traffic_state, traffic_done_tx)
            .await?;

        // Step 4: Set ingress traffic to 100%
        let percent = WholePercent::try_from(100)
            .map_err(|e| miette!("Failed to create WholePercent from 100: {:?}", e))?;
        self.ingress.set_canary_traffic(percent).await?;

        // Mark SET_CANARY_TRAFFIC state as DONE
        self.backend
            .mark_state_completed(&self.meta, &locked_traffic_state)
            .await?;

        // Step 5: Promote the canary to production in ingress and platform
        self.ingress.promote_canary().await?;
        self.platform.promote_rollout().await?;

        // Step 6: Update the rollout status to Promoted
        self.backend
            .update_rollout(&self.meta, RolloutStatus::Promoted)
            .await?;

        Ok(())
    }
}
