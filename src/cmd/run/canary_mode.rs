use async_trait::async_trait;
use miette::Result;
use tokio::signal;
use tracing::{debug, info};

use crate::adapters::{BackendClient, BoxedIngress, BoxedMonitor, BoxedPlatform, RolloutMetadata};
use crate::ControllerSubsystem;

use super::DeploymentMode;

/// Canary deployment mode - runs the full canary analysis subsystems
pub struct CanaryMode {
    backend: BackendClient,
    monitor: BoxedMonitor,
    ingress: BoxedIngress,
    platform: BoxedPlatform,
    meta: RolloutMetadata,
}

impl CanaryMode {
    pub fn new(
        backend: BackendClient,
        monitor: BoxedMonitor,
        ingress: BoxedIngress,
        platform: BoxedPlatform,
        meta: RolloutMetadata,
    ) -> Self {
        Self {
            backend,
            monitor,
            ingress,
            platform,
            meta,
        }
    }
}

#[async_trait]
impl DeploymentMode for CanaryMode {
    async fn dispatch(self: Box<Self>) -> Result<()> {
        // Build the ControllerSubsystem using the boxed objects.
        debug!("Building controller...");
        let controller = ControllerSubsystem::builder()
            .backend(self.backend)
            .monitor(self.monitor)
            .ingress(self.ingress)
            .platform(self.platform)
            .meta(self.meta)
            .build();

        info!("Starting the rollout...");

        // Spawn the controller actor
        let controller_ref = controller.spawn();

        // Wait for either the controller to finish or a shutdown signal
        tokio::select! {
            // Wait for the controller to stop (either normally or due to error)
            _ = controller_ref.wait_for_shutdown() => {
                debug!("Controller stopped");
            }
            // Handle Ctrl+C signal
            _ = signal::ctrl_c() => {
                info!("Received shutdown signal, stopping...");
                if let Err(e) = controller_ref.stop_gracefully().await {
                    debug!("Error during graceful shutdown: {:?}", e);
                }
                // Wait for the actor to finish shutting down
                controller_ref.wait_for_shutdown().await;
                debug!("Controller stopped after shutdown signal");
            }
        }

        Ok(())
    }
}
