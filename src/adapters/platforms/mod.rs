use async_trait::async_trait;
use miette::Result;
use mockall::automock;

use crate::{Shutdownable, subsystems::ShutdownResult};
pub type BoxedPlatform = Box<dyn Platform + Send + Sync>;

pub(crate) use cloudflare::CloudflareWorkerPlatform;
pub(crate) use lambda::LambdaPlatform;

use super::backend::PlatformConfig;

#[automock]
#[async_trait]
pub trait Platform: Shutdownable {
    /// Returns the configuration data for this platform
    fn get_config(&self) -> PlatformConfig;
    /// Deploy the canary app. Do not assign it any traffic.
    async fn deploy(&mut self) -> Result<(String, String)>;
    /// Remove the canary app from the platform.
    async fn yank_canary(&mut self) -> Result<()>;
    /// Delete the canary app from the platform.
    /// This is slightly different than Yank becuase it actually
    /// destroys the resoruces we created.
    async fn delete_canary(&mut self) -> Result<()>;
    /// Make the canary app the new baseline.
    async fn promote_rollout(&mut self) -> Result<()>;
}

#[async_trait]
impl Shutdownable for MockPlatform {
    async fn shutdown(&mut self) -> ShutdownResult {
        Ok(())
    }
}

mod cloudflare;
mod lambda;

#[cfg(test)]
mod tests {
    use super::Platform;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Platform);
}
