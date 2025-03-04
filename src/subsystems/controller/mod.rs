use async_trait::async_trait;
use bon::bon;
use miette::{Report, Result, bail};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle};

use crate::adapters::{
    BackendClient, BoxedIngress, BoxedMonitor, BoxedPlatform, DeploymentMetadata,
};
use crate::subsystems::PLATFORM_SUBSYSTEM_NAME;
use crate::{IngressSubsystem, PlatformSubsystem};

use monitor::{MONITOR_CONTROLLER_SUBSYSTEM_NAME, MonitorController};

use super::{INGRESS_SUBSYSTEM_NAME, RELAY_SUBSYSTEM_NAME, RelaySubsystem};

/// This is the name as reported to the `TopLevelSubsystem`,
/// presumably for logging.
pub const CONTROLLER_SUBSYSTEM_NAME: &str = "controller";

/// The [ControllerSubsystem] is responsible for talking to the backend.
/// It sends new monitoring observations, asks for instructions to perform
/// on cloud resources, and reports the state of those instructions back
/// to the backend.
pub struct ControllerSubsystem {
    backend: BackendClient,
    monitor: BoxedMonitor,
    ingress: BoxedIngress,
    platform: BoxedPlatform,
    /// This field contains context about the current deployment
    /// and is frequently passed to the backend.
    meta: DeploymentMetadata,
}

#[bon]
impl ControllerSubsystem {
    #[builder]
    pub fn new(
        backend: BackendClient,
        monitor: BoxedMonitor,
        ingress: BoxedIngress,
        platform: BoxedPlatform,
        meta: DeploymentMetadata,
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
impl IntoSubsystem<Report> for ControllerSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        let ingress_subsystem = IngressSubsystem::new(self.ingress);
        let ingress_handle = ingress_subsystem.handle();

        let platform_subsystem = PlatformSubsystem::new(self.platform);
        let platform_handle = platform_subsystem.handle();

        let mut monitor_controller = MonitorController::builder().monitor(self.monitor).build();
        let observation_stream = match monitor_controller.stream() {
            Some(stream) => stream,
            None => {
                bail!(
                    "Failed to take monitoring stream. This is an internal error and should be reported as a bug."
                );
            }
        };

        let relay_subsystem = RelaySubsystem::builder()
            .backend(self.backend)
            .observations(observation_stream)
            .platform(platform_handle)
            .ingress(ingress_handle)
            .meta(self.meta)
            .build();

        // • Start the ingress subsystem.
        subsys.start(SubsystemBuilder::new(
            INGRESS_SUBSYSTEM_NAME,
            ingress_subsystem.into_subsystem(),
        ));

        // • Start the platform subsystem.
        subsys.start(SubsystemBuilder::new(
            PLATFORM_SUBSYSTEM_NAME,
            platform_subsystem.into_subsystem(),
        ));

        // • Start the MonitorController subsytem.
        subsys.start(SubsystemBuilder::new(
            MONITOR_CONTROLLER_SUBSYSTEM_NAME,
            monitor_controller.into_subsystem(),
        ));

        // • Start the relay subsystem.
        subsys.start(SubsystemBuilder::new(
            RELAY_SUBSYSTEM_NAME,
            relay_subsystem.into_subsystem(),
        ));

        subsys.wait_for_children().await;
        Ok(())
    }
}

/// Contains the controller for the monitor, controlling how
/// often it gets called.
mod monitor;

#[cfg(test)]
mod tests {
    use crate::{metrics::ResponseStatusCode, stats::CategoricalObservation};

    use super::ControllerSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(ControllerSubsystem: IntoSubsystem<Report>);
}
