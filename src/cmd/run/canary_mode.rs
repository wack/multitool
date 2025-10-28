use async_trait::async_trait;
use miette::Result;
use tokio::time::Duration;
use tokio_graceful_shutdown::{IntoSubsystem as _, SubsystemBuilder, Toplevel};
use tracing::{debug, info};

use crate::ControllerSubsystem;
use crate::adapters::{BackendClient, BoxedIngress, BoxedMonitor, BoxedPlatform, RolloutMetadata};
use crate::subsystems::CONTROLLER_SUBSYSTEM_NAME;

use super::{DEFAULT_SHUTDOWN_TIMEOUT, DeploymentMode};

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

        // Let's capture the shutdown signal from the OS.
        Toplevel::new(|s| async move {
            // • Start the action listener subsystem.
            s.start(SubsystemBuilder::new(
                CONTROLLER_SUBSYSTEM_NAME,
                controller.into_subsystem(),
            ));
        })
        .catch_signals()
        .handle_shutdown_requests(Duration::from_millis(DEFAULT_SHUTDOWN_TIMEOUT))
        .await
        .map_err(Into::into)
    }
}
