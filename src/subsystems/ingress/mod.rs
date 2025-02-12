use async_trait::async_trait;
use miette::{Report, Result};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::adapters::BoxIngress;

pub struct IngressSubsystem;

impl IngressSubsystem {
    pub fn new(_ingress: BoxIngress) -> Self {
        Self
    }
}

pub const INGRESS_SUBSYSTEM_NAME: &str = "ingress";

#[async_trait]
impl IntoSubsystem<Report> for IngressSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::IngressSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(IngressSubsystem: IntoSubsystem<Report>);
}
