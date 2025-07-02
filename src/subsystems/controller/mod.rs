use async_trait::async_trait;
use bon::bon;
use miette::{Report, Result};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle};
use tracing::{debug, error, trace};

use crate::adapters::{BackendClient, BoxedIngress, BoxedMonitor, BoxedPlatform, RolloutMetadata};
use crate::subsystems::PLATFORM_SUBSYSTEM_NAME;
use crate::{IngressSubsystem, PlatformSubsystem};

#[cfg(feature = "errorlogs")]
use crate::subsystems::{ERROR_LOGS_SUBSYSTEM_NAME, ErrorLogsController};
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
    /// This field contains context about the current rollout
    /// and is frequently passed to the backend.
    meta: RolloutMetadata,
}

#[bon]
impl ControllerSubsystem {
    #[builder]
    pub fn new(
        backend: BackendClient,
        monitor: BoxedMonitor,
        ingress: BoxedIngress,
        platform: BoxedPlatform,
        meta: RolloutMetadata,
    ) -> Self {
        trace!("Creating a new controller subsystem...");

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
        debug!("Running the controller subsystem...");
        let ingress_subsystem = IngressSubsystem::new(self.ingress);
        let ingress_handle = ingress_subsystem.handle();

        let platform_subsystem = PlatformSubsystem::new(self.platform);
        let platform_handle = platform_subsystem.handle();

        // Extract monitor config before moving the monitor so we can use it
        // in the ErrorLogsController.
        #[cfg(feature = "errorlogs")]
        let monitor_config = self.monitor.get_config();

        let mut monitor_controller = MonitorController::builder().monitor(self.monitor).build();
        let observation_stream = monitor_controller.stream()?;

        let baseline_sender = monitor_controller.get_baseline_sender();
        let canary_sender = monitor_controller.get_canary_sender();

        let relay_subsystem = RelaySubsystem::builder()
            .backend(self.backend)
            .observations(observation_stream)
            .platform(platform_handle)
            .ingress(ingress_handle)
            .meta(self.meta)
            .baseline_sender(baseline_sender)
            .canary_sender(canary_sender)
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

        #[cfg(feature = "errorlogs")]
        {
            match ErrorLogsController::builder()
                .monitor(monitor_config)
                .build()
            {
                Ok(error_logs_controller) => {
                    subsys.start(SubsystemBuilder::new(
                        ERROR_LOGS_SUBSYSTEM_NAME,
                        error_logs_controller.into_subsystem(),
                    ));
                }
                Err(err) => {
                    error!("Could not start error logs subsystem: {}", err);
                }
            }
        }

        subsys.wait_for_children().await;
        Ok(())
    }
}

/// Contains the controller for the monitor, controlling how
/// often it gets called.
mod monitor;

#[cfg(test)]
mod tests {
    use super::ControllerSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(ControllerSubsystem: IntoSubsystem<Report>);
}
