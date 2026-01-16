use async_trait::async_trait;
use kameo::actor::{ActorRef, Spawn};
use kameo::error::SendError;
use kameo::mailbox;
use kameo::message::{Context, Message};
use kameo::Actor;
use miette::Result;
use tracing::debug;

use crate::adapters::backend::IngressConfig;
use crate::adapters::{BoxedIngress, Ingress};
use crate::subsystems::{ShutdownResult, Shutdownable};
use crate::WholePercent;

#[allow(dead_code)]
pub const INGRESS_SUBSYSTEM_NAME: &str = "ingress";

/// The IngressSubsystem handles synchronizing access to the
/// `BoxedIngress` using Kameo's actor model.
#[derive(Actor)]
pub struct IngressSubsystem {
    ingress: BoxedIngress,
}

impl IngressSubsystem {
    pub fn new(ingress: BoxedIngress) -> Self {
        Self { ingress }
    }

    /// Spawn the actor and return a handle (ActorRef wrapped in a BoxedIngress).
    pub fn spawn_boxed(ingress: BoxedIngress) -> BoxedIngress {
        let actor = Self::new(ingress);
        let actor_ref = Self::spawn_with_mailbox(actor, mailbox::unbounded());
        Box::new(IngressHandle::new(actor_ref))
    }
}

// --- Messages ---

/// Message to release the canary.
pub struct ReleaseCanary {
    pub baseline_version_id: String,
    pub canary_version_id: String,
}

impl Message<ReleaseCanary> for IngressSubsystem {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: ReleaseCanary,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ingress
            .release_canary(msg.baseline_version_id, msg.canary_version_id)
            .await
    }
}

/// Message to set canary traffic percentage.
pub struct SetCanaryTraffic {
    pub percent: WholePercent,
}

impl Message<SetCanaryTraffic> for IngressSubsystem {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: SetCanaryTraffic,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ingress.set_canary_traffic(msg.percent).await
    }
}

/// Message to rollback the canary.
pub struct RollbackCanary;

impl Message<RollbackCanary> for IngressSubsystem {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        _msg: RollbackCanary,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ingress.rollback_canary().await
    }
}

/// Message to promote the canary.
pub struct PromoteCanary;

impl Message<PromoteCanary> for IngressSubsystem {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        _msg: PromoteCanary,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.ingress.promote_canary().await
    }
}

/// Message to shutdown the ingress.
pub struct Shutdown;

impl Message<Shutdown> for IngressSubsystem {
    type Reply = ShutdownResult;

    async fn handle(
        &mut self,
        _msg: Shutdown,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("Shutting down ingress subsystem");
        self.ingress.shutdown().await
    }
}

// --- Handle (wraps ActorRef to implement Ingress trait) ---

/// A handle to the IngressSubsystem actor that implements the Ingress trait.
#[derive(Clone)]
pub struct IngressHandle {
    actor_ref: ActorRef<IngressSubsystem>,
}

impl IngressHandle {
    pub fn new(actor_ref: ActorRef<IngressSubsystem>) -> Self {
        Self { actor_ref }
    }

    /// Get the underlying actor reference.
    #[allow(dead_code)]
    pub fn actor_ref(&self) -> &ActorRef<IngressSubsystem> {
        &self.actor_ref
    }
}

#[async_trait]
impl Ingress for IngressHandle {
    async fn release_canary(
        &mut self,
        baseline_version_id: String,
        canary_version_id: String,
    ) -> Result<()> {
        self.actor_ref
            .ask(ReleaseCanary {
                baseline_version_id,
                canary_version_id,
            })
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to ingress: {:?}", e))?;
        Ok(())
    }

    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        self.actor_ref
            .ask(SetCanaryTraffic { percent })
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to ingress: {:?}", e))?;
        Ok(())
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        self.actor_ref
            .ask(RollbackCanary)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to ingress: {:?}", e))?;
        Ok(())
    }

    async fn promote_canary(&mut self) -> Result<()> {
        self.actor_ref
            .ask(PromoteCanary)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to ingress: {:?}", e))?;
        Ok(())
    }

    fn get_config(&self) -> IngressConfig {
        panic!(
            "This should never be called, as the IngressHandle is a handle to an ingress that is already running."
        )
    }
}

#[async_trait]
impl Shutdownable for IngressHandle {
    async fn shutdown(&mut self) -> ShutdownResult {
        self.actor_ref
            .ask(Shutdown)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to shutdown ingress: {:?}", e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::IngressSubsystem;
    use kameo::Actor;
    use static_assertions::assert_impl_all;

    assert_impl_all!(IngressSubsystem: Actor);
}
