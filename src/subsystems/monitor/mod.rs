use async_trait::async_trait;
use kameo::actor::{ActorRef, Spawn};
use kameo::error::SendError;
use kameo::mailbox;
use kameo::message::{Context, Message};
use kameo::Actor;
use miette::Result;
use tracing::debug;

use crate::adapters::backend::MonitorConfig;
use crate::adapters::{BoxedMonitor, Monitor, StatusCode};
use crate::subsystems::{ShutdownResult, Shutdownable};

#[allow(dead_code)]
pub const MONITOR_SUBSYSTEM_NAME: &str = "monitor";

/// The MonitorSubsystem handles synchronizing access to the
/// `BoxedMonitor` using Kameo's actor model.
#[derive(Actor)]
pub struct MonitorSubsystem {
    monitor: BoxedMonitor,
}

impl MonitorSubsystem {
    pub fn new(monitor: BoxedMonitor) -> Self {
        Self { monitor }
    }

    /// Spawn the actor and return a handle (ActorRef wrapped in a BoxedMonitor).
    pub fn spawn_boxed(monitor: BoxedMonitor) -> BoxedMonitor {
        let actor = Self::new(monitor);
        let actor_ref = Self::spawn_with_mailbox(actor, mailbox::unbounded());
        Box::new(MonitorHandle::new(actor_ref))
    }

    /// Create the actor and return the actor reference directly.
    /// This is useful when you need direct access to the actor ref.
    pub fn spawn_actor(monitor: BoxedMonitor) -> ActorRef<Self> {
        let actor = Self::new(monitor);
        Self::spawn_with_mailbox(actor, mailbox::unbounded())
    }
}

// --- Messages ---

/// Message to query the monitor for observations.
pub struct Query;

impl Message<Query> for MonitorSubsystem {
    type Reply = Result<Vec<StatusCode>>;

    async fn handle(
        &mut self,
        _msg: Query,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.monitor.query().await
    }
}

/// Message to set the baseline version ID.
pub struct SetBaselineVersionId {
    pub version_id: String,
}

impl Message<SetBaselineVersionId> for MonitorSubsystem {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: SetBaselineVersionId,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.monitor.set_baseline_version_id(msg.version_id).await
    }
}

/// Message to set the canary version ID.
pub struct SetCanaryVersionId {
    pub version_id: String,
}

impl Message<SetCanaryVersionId> for MonitorSubsystem {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: SetCanaryVersionId,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.monitor.set_canary_version_id(msg.version_id).await
    }
}

/// Message to shutdown the monitor.
pub struct Shutdown;

impl Message<Shutdown> for MonitorSubsystem {
    type Reply = ShutdownResult;

    async fn handle(
        &mut self,
        _msg: Shutdown,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        debug!("Shutting down monitor subsystem");
        self.monitor.shutdown().await
    }
}

// --- Handle (wraps ActorRef to implement Monitor trait) ---

/// A handle to the MonitorSubsystem actor that implements the Monitor trait.
#[derive(Clone)]
pub struct MonitorHandle {
    actor_ref: ActorRef<MonitorSubsystem>,
}

impl MonitorHandle {
    pub fn new(actor_ref: ActorRef<MonitorSubsystem>) -> Self {
        Self { actor_ref }
    }

    /// Get the underlying actor reference.
    #[allow(dead_code)]
    pub fn actor_ref(&self) -> &ActorRef<MonitorSubsystem> {
        &self.actor_ref
    }
}

#[async_trait]
impl Monitor for MonitorHandle {
    type Item = StatusCode;

    async fn query(&mut self) -> Result<Vec<StatusCode>> {
        self.actor_ref
            .ask(Query)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to monitor: {:?}", e))
    }

    async fn set_baseline_version_id(&mut self, version_id: String) -> Result<()> {
        self.actor_ref
            .ask(SetBaselineVersionId { version_id })
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to monitor: {:?}", e))?;
        Ok(())
    }

    async fn set_canary_version_id(&mut self, version_id: String) -> Result<()> {
        self.actor_ref
            .ask(SetCanaryVersionId { version_id })
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to send to monitor: {:?}", e))?;
        Ok(())
    }

    fn get_config(&self) -> MonitorConfig {
        panic!(
            "This should never be called, as the MonitorHandle is a handle to a monitor that is already running."
        )
    }
}

#[async_trait]
impl Shutdownable for MonitorHandle {
    async fn shutdown(&mut self) -> ShutdownResult {
        self.actor_ref
            .ask(Shutdown)
            .await
            .map_err(|e: SendError<_, _>| miette::miette!("Failed to shutdown monitor: {:?}", e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::MonitorSubsystem;
    use kameo::Actor;
    use static_assertions::assert_impl_all;

    assert_impl_all!(MonitorSubsystem: Actor);
}
