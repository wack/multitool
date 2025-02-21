use std::sync::Arc;

use async_trait::async_trait;
use miette::{Report, Result};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::adapters::BackendClient;

pub struct ControllerSubsystem {
    backend: Arc<dyn BackendClient + 'static>,
}

impl ControllerSubsystem {
    pub fn new(backend: Arc<dyn BackendClient>) -> Self {
        Self { backend }
    }
}

pub const CONTROLLER_SUBSYSTEM_NAME: &str = "controller";

#[async_trait]
impl IntoSubsystem<Report> for ControllerSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        // Spawn a thread that calls the monitor on a timer.
        //   * Convert the results into a stream.
        //   * Consume the stream in a thread and push the results
        //     to the backend.
        // Poll the backend for new states to effect.
        //   * Spawn a thread that runs on a timer.
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::ControllerSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(ControllerSubsystem: IntoSubsystem<Report>);
}
