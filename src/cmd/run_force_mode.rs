use async_trait::async_trait;
use miette::{Result, miette};
use multitool_sdk::models::{RolloutStateStatus, RolloutStateType, RolloutStatus};
use tokio::time::{Duration, sleep};
use tracing::trace;

use crate::WholePercent;
use crate::adapters::{BackendClient, BoxedIngress, BoxedMonitor, BoxedPlatform, RolloutMetadata};

use crate::cmd::run::DeploymentMode;

/// Force deployment mode - bypasses canary analysis and immediately deploys at 100%
pub struct ForceMode;

const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[async_trait]
impl DeploymentMode for ForceMode {
    async fn handle(
        backend: BackendClient,
        _monitor: BoxedMonitor,
        mut ingress: BoxedIngress,
        mut platform: BoxedPlatform,
        meta: RolloutMetadata,
    ) -> Result<()> {
        // Step 1: Poll for DEPLOY_CANARY state
        trace!("Waiting for DEPLOY_CANARY state...");
        let deploy_state = loop {
            let states = backend.poll_for_state(&meta).await?;

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
        let locked_deploy_state = backend.lock_state_sync(&meta, &deploy_state).await?;

        // Step 2: Deploy the platform
        let (baseline_version_id, canary_version_id) = platform.deploy().await?;

        // Release the canary to the ingress with 0% traffic initially
        ingress
            .release_canary(baseline_version_id, canary_version_id)
            .await?;

        // Mark DEPLOY_CANARY state as DONE
        backend
            .mark_state_completed_sync(&meta, &locked_deploy_state)
            .await?;

        // Step 3: Poll for SET_CANARY_TRAFFIC state (to 100%)
        trace!("Waiting for SET_CANARY_TRAFFIC state...");
        let traffic_state = loop {
            let states = backend.poll_for_state(&meta).await?;

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
        let locked_traffic_state = backend.lock_state_sync(&meta, &traffic_state).await?;

        // Step 4: Set ingress traffic to 100%
        let percent = WholePercent::try_from(100)
            .map_err(|e| miette!("Failed to create WholePercent from 100: {:?}", e))?;
        ingress.set_canary_traffic(percent).await?;

        // Mark SET_CANARY_TRAFFIC state as DONE
        backend
            .mark_state_completed_sync(&meta, &locked_traffic_state)
            .await?;

        // Step 5: Promote the canary to production in ingress and platform
        ingress.promote_canary().await?;
        platform.promote_rollout().await?;

        // Step 6: Update the rollout status to Promoted
        backend
            .update_rollout(&meta, RolloutStatus::Promoted)
            .await?;

        Ok(())
    }
}
