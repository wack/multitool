use async_trait::async_trait;
use kameo::actor::{ActorRef, Spawn};
use kameo::error::SendError;
use kameo::mailbox;
use kameo::message::{Context, Message};
use kameo::Actor;
use miette::Result;
use tracing::debug;

use crate::adapters::backend::PlatformConfig;
use crate::adapters::{BoxedPlatform, Platform};
use crate::subsystems::{ShutdownResult, Shutdownable};

#[allow(dead_code)]
pub const PLATFORM_SUBSYSTEM_NAME: &str = "platform";

/// The PlatformSubsystem handles synchronizing access to the
/// `BoxedPlatform` using Kameo's actor model.
#[derive(Actor)]
pub struct PlatformSubsystem {
    platform: BoxedPlatform,
}

impl PlatformSubsystem {
    pub fn new(platform: BoxedPlatform) -> Self {
        Self { platform }
    }

    /// Spawn the actor and return a handle (ActorRef wrapped in a BoxedPlatform).
    pub fn spawn_boxed(platform: BoxedPlatform) -> BoxedPlatform {
        let actor_ref = Self::spawn_with_mailbox(Self::new(platform), mailbox::unbounded());
        Box::new(PlatformHandle::new(actor_ref))
    }
}

// --- Messages ---

/// Message to deploy the canary.
pub struct DeployCanary;

impl Message<DeployCanary> for PlatformSubsystem {
    type Reply = Result<(String, String)>;

    async fn handle(
        &mut self,
        _msg: DeployCanary,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.platform.deploy().await
    }
}

/// Message to yank the canary.
pub struct YankCanary;

impl Message<YankCanary> for PlatformSubsystem {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        _msg: YankCanary,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.platform.yank_canary().await
    }
}

/// Message to delete the canary.
pub struct DeleteCanary;

impl Message<DeleteCanary> for PlatformSubsystem {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        _msg: DeleteCanary,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.platform.delete_canary().await
    }
}

/// Message to promote the rollout.
pub struct PromoteRollout;

impl Message<PromoteRollout> for PlatformSubsystem {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        _msg: PromoteRollout,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // Note: original code called yank_canary here, preserving that behavior
        self.platform.yank_canary().await
    }
}

/// Message to shutdown the platform.
pub struct Shutdown;

impl Message<Shutdown> for PlatformSubsystem {
    type Reply = ShutdownResult;

    async fn handle(
        &mut self,
        _msg: Shutdown,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("Shutting down platform subsystem");
        self.platform.shutdown().await
    }
}

// --- Handle (wraps ActorRef to implement Platform trait) ---

/// A handle to the PlatformSubsystem actor that implements the Platform trait.
#[derive(Clone)]
pub struct PlatformHandle {
    actor_ref: ActorRef<PlatformSubsystem>,
}

impl PlatformHandle {
    pub fn new(actor_ref: ActorRef<PlatformSubsystem>) -> Self {
        Self { actor_ref }
    }

    /// Get the underlying actor reference.
    #[allow(dead_code)]
    pub fn actor_ref(&self) -> &ActorRef<PlatformSubsystem> {
        &self.actor_ref
    }
}

#[async_trait]
impl Platform for PlatformHandle {
    async fn deploy(&mut self) -> Result<(String, String)> {
        self.actor_ref
            .ask(DeployCanary)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to platform: {:?}", e))
    }

    async fn yank_canary(&mut self) -> Result<()> {
        self.actor_ref
            .ask(YankCanary)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to platform: {:?}", e))?;
        Ok(())
    }

    async fn delete_canary(&mut self) -> Result<()> {
        self.actor_ref
            .ask(DeleteCanary)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to platform: {:?}", e))?;
        Ok(())
    }

    async fn promote_rollout(&mut self) -> Result<()> {
        self.actor_ref
            .ask(PromoteRollout)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to platform: {:?}", e))?;
        Ok(())
    }

    fn get_config(&self) -> PlatformConfig {
        panic!(
            "This should never be called, as the PlatformHandle is a handle to a platform that is already running."
        )
    }
}

#[async_trait]
impl Shutdownable for PlatformHandle {
    async fn shutdown(&mut self) -> ShutdownResult {
        self.actor_ref
            .ask(Shutdown)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to shutdown platform: {:?}", e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::adapters::MockPlatform;
    use crate::adapters::Platform;

    use super::{PlatformHandle, PlatformSubsystem};
    use kameo::Actor;
    use miette::Result;
    use static_assertions::assert_impl_all;

    assert_impl_all!(PlatformSubsystem: Actor);
    assert_impl_all!(PlatformHandle: Platform, Clone);

    /// This test demonstrates how to use the PlatformSubsystem.
    #[tokio::test]
    async fn use_platform_subsystem() -> Result<()> {
        // Construct a mock platform to provide to the subsystem.
        let mut mock_platform = MockPlatform::new();
        mock_platform.expect_yank_canary().returning(|| Ok(()));

        // Spawn the actor and get a handle
        let mut handle = PlatformSubsystem::spawn_boxed(Box::new(mock_platform));

        // Yank the canary.
        assert!(handle.yank_canary().await.is_ok());

        Ok(())
    }
}
