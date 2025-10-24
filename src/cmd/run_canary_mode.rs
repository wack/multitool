use async_trait::async_trait;
use miette::Result;
use tokio::time::Duration;
use tokio_graceful_shutdown::{IntoSubsystem as _, SubsystemBuilder, Toplevel};
use tracing::{debug, info};

use crate::ControllerSubsystem;
use crate::adapters::{BackendClient, BoxedIngress, BoxedMonitor, BoxedPlatform, RolloutMetadata};
use crate::subsystems::CONTROLLER_SUBSYSTEM_NAME;

use crate::cmd::run::{DEFAULT_SHUTDOWN_TIMEOUT, DeploymentMode};

/// Canary deployment mode - runs the full canary analysis subsystems
pub struct CanaryMode;

#[async_trait]
impl DeploymentMode for CanaryMode {
    async fn handle(
        backend: BackendClient,
        monitor: BoxedMonitor,
        ingress: BoxedIngress,
        platform: BoxedPlatform,
        meta: RolloutMetadata,
    ) -> Result<()> {
        // Build the ControllerSubsystem using the boxed objects.
        debug!("Building controller...");
        let controller = ControllerSubsystem::builder()
            .backend(backend)
            .monitor(monitor)
            .ingress(ingress)
            .platform(platform)
            .meta(meta)
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
