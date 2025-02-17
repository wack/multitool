use std::sync::Arc;

use async_trait::async_trait;
use miette::{IntoDiagnostic, Report, Result};
use tokio::{
    sync::{
        mpsc::{self, Sender},
        oneshot,
    },
    task::JoinHandle,
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::{
    adapters::{BoxedPlatform, Platform},
    artifacts::LambdaZip,
};

use super::mail::{DeployParams, PlatformMail, PromoteParams, RollbackParams};

/// A `[PlatformHandle]` provides access to all of the methods
/// on a Platform, but synchronizes them via message-passing.
#[derive(Clone)]
pub struct PlatformHandle {
    outbox: Arc<Sender<PlatformMail>>,
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
    pub fn new(outbox: Arc<Sender<PlatformMail>>) -> Self {
        Self { outbox }
    }
}
