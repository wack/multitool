use async_trait::async_trait;
use miette::{Report, Result};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::{adapters::BoxPlatform, artifacts::LambdaZip};

pub const PLATFORM_SUBSYSTEM_NAME: &str = "platform";

pub struct PlatformSubsystem {
    artifact: LambdaZip,
}

impl PlatformSubsystem {
    pub fn new(artifact: LambdaZip, _platform: BoxPlatform) -> Self {
        Self { artifact }
    }
}

#[async_trait]
impl IntoSubsystem<Report> for PlatformSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::PlatformSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(PlatformSubsystem: IntoSubsystem<Report>);
}
