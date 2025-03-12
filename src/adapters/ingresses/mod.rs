use async_trait::async_trait;
use miette::Result;

use crate::{Shutdownable, WholePercent};

/// Convenience alias since this type is often dynamically
/// dispatched.
pub type BoxedIngress = Box<dyn Ingress + Send + Sync>;

pub(super) use builder::IngressBuilder;

/// Ingresses are responsible for (1) controlling how much traffic the canary
/// gets (hence the name ingress, since it functions like a virtual LB) and
/// (2) deploying, yanking, and promoting both the canary and the baseline.
#[async_trait]
pub trait Ingress: Shutdownable {
    /// Given a deployed platform, release the canary in the ingress.
    async fn release_canary(&mut self, platform_id: String) -> Result<()>;
    /// The `[Ingress]` controls how much traffic the canary gets.
    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()>;
    /// This method is subtly different from `Platform::yank_canary`.
    /// The `[Ingress]` may be responsible for cutting traffic to the canary
    /// in the event of a rollback. Unlike `Platform::yank_canary`,
    /// which removes the canary from the platform, this method
    /// cuts traffic to the canary and removes it from the ingress,
    /// but it doesn't remove the deployment from the platform.
    ///
    /// In the "build, deploy, release" model of application lifecycle,
    /// this is effectively cutting the release without affecting the
    /// deployment.
    async fn rollback_canary(&mut self) -> Result<()>;
    /// This method promotes the canary to the new baseline within
    /// the context of the ingress. It does not affect the underlying
    /// deployment.
    async fn promote_canary(&mut self) -> Result<()>;
}

mod apig;
mod builder;

#[cfg(test)]
mod tests {
    use super::Ingress;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Ingress);
}
