use async_trait::async_trait;
use miette::{Report, Result};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

pub const MONITOR_SUBSYSTEM_NAME: &str = "monitor";

pub struct MonitorSubsystem;

mod handle;
mod mail;

#[async_trait]
impl IntoSubsystem<Report> for MonitorSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::MonitorSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(MonitorSubsystem: IntoSubsystem<Report>);
}
