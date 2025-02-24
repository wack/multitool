use async_trait::async_trait;
use miette::Result;

pub use builder::PlatformBuilder;
pub use lambda::LambdaPlatform;

use crate::Shutdownable;
pub type BoxedPlatform = Box<dyn Platform + Send + Sync>;

#[async_trait]
pub trait Platform: Shutdownable {
    /// Deploy the canary app. Do not assign it any traffic.
    async fn deploy(&mut self) -> Result<()>;
    /// Remove the canary app from the platform.
    async fn yank_canary(&mut self) -> Result<()>;
    /// Make the canary app the new baseline.
    async fn promote_deployment(&mut self) -> Result<()>;
}

mod builder;
mod lambda;

#[cfg(test)]
mod tests {
    use super::Platform;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Platform);
}
