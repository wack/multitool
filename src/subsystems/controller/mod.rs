use std::sync::Arc;

use async_trait::async_trait;
use miette::{Report, Result};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle};

use crate::adapters::{BackendClient, BoxedIngress, BoxedMonitor, BoxedPlatform};
use crate::stats::Observation;
use crate::subsystems::PLATFORM_SUBSYSTEM_NAME;
use crate::{IngressSubsystem, PlatformSubsystem};

use monitor::{MONITOR_CONTROLLER_SUBSYSTEM_NAME, MonitorController};

use super::INGRESS_SUBSYSTEM_NAME;

/// This is the name as reported to the `TopLevelSubsystem`,
/// presumably for logging.
pub const CONTROLLER_SUBSYSTEM_NAME: &str = "controller";

/// The [ControllerSubsystem] is responsible for talking to the backend.
/// It sends new monitoring observations, asks for instructions to perform
/// on cloud resources, and reports the state of those instructions back
/// to the backend.
pub struct ControllerSubsystem<T: Observation> {
    backend: Arc<dyn BackendClient + 'static>,
    monitor: BoxedMonitor<T>,
    ingress: BoxedIngress,
    platform: BoxedPlatform,
}

impl<T: Observation> ControllerSubsystem<T> {
    pub fn new(
        backend: Arc<dyn BackendClient>,
        monitor: BoxedMonitor<T>,
        ingress: BoxedIngress,
        platform: BoxedPlatform,
    ) -> Self {
        Self {
            backend,
            monitor,
            ingress,
            platform,
        }
    }
}

#[async_trait]
impl<T: Observation + Clone + Send + 'static> IntoSubsystem<Report> for ControllerSubsystem<T> {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        let ingress_subsystem = IngressSubsystem::new(self.ingress);
        let ingress_handle = ingress_subsystem.handle();

        let platform_subsystem = PlatformSubsystem::new(self.platform);
        let platform_handle = platform_subsystem.handle();

        let monitor_controller = MonitorController::new(self.monitor);
        let observation_stream = monitor_controller.subscribe();

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

        subsys.wait_for_children().await;

        // Spawn a thread that calls the monitor on a timer.
        //   * Convert the results into a stream.
        //   * Consume the stream in a thread and push the results
        //     to the backend.
        // Poll the backend for new states to effect.
        //   * Spawn a thread that runs on a timer.
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

    assert_impl_all!(ControllerSubsystem<CategoricalObservation<5, ResponseStatusCode>>: IntoSubsystem<Report>);
}
