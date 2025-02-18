use std::sync::Arc;

use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot,
};

use crate::{adapters::Platform, subsystems::ShutdownResult, Shutdownable};

use super::mail::{DeployParams, PlatformMail, PromoteParams, RollbackParams};

/// A `[PlatformHandle]` provides access to all of the methods
/// on a Platform, but synchronizes them via message-passing.
#[derive(Clone)]
pub struct PlatformHandle {
    outbox: Arc<Sender<PlatformMail>>,
    shutdown_trigger: Arc<mpsc::Sender<()>>,
}

#[async_trait]
impl Platform for PlatformHandle {
    async fn deploy(&mut self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = DeployParams::new(sender);
        let mail = PlatformMail::DeployCanary(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = RollbackParams::new(sender);
        let mail = PlatformMail::RollbackCanary(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }

    async fn promote_canary(&mut self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = PromoteParams::new(sender);
        let mail = PlatformMail::PromoteCanary(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
}

impl PlatformHandle {
    pub(super) fn new(
        outbox: Arc<Sender<PlatformMail>>,
        shutdown_trigger: Arc<mpsc::Sender<()>>,
    ) -> Self {
        Self {
            outbox,
            shutdown_trigger,
        }
    }
}

#[async_trait]
impl Shutdownable for PlatformHandle {
    async fn shutdown(&mut self) -> ShutdownResult {
        self.shutdown_trigger.send(()).await.into_diagnostic()
    }
}

#[cfg(test)]
mod tests {
    use super::PlatformHandle;
    use crate::adapters::Platform;

    use static_assertions::assert_impl_all;

    assert_impl_all!(PlatformHandle: Platform);
}
