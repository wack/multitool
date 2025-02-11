use async_trait::async_trait;
use miette::{Report, Result};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

pub struct ActionListenerSubsystem;

pub const ACTION_LISTENER_SUBSYSTEM_NAME: &str = "action-listener";

#[async_trait]
impl IntoSubsystem<Report> for ActionListenerSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::ActionListenerSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(ActionListenerSubsystem: IntoSubsystem<Report>);
}
