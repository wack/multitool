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

pub const PLATFORM_SUBSYSTEM_NAME: &str = "platform";
/// if you're going to pick an arbirary number, you could do worse
/// than picking a power of two.
const PLATFORM_MAILBOX_SIZE: usize = 1 << 4;

/// The PlatformSubsystem handles sychronizing access to the
/// `[BoxedPlatform]` using message-passing and channels.
pub struct PlatformSubsystem {
    artifact: LambdaZip,
    handle: PlatformHandle,
    task_done: JoinHandle<()>,
}

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

impl PlatformSubsystem {
    pub fn new(artifact: LambdaZip, mut platform: BoxedPlatform) -> Self {
        // • Spawn a new task with the BoxedPlatform and the mailbox.
        let (outbox, mut inbox) = mpsc::channel(PLATFORM_MAILBOX_SIZE);
        let join_handle = tokio::spawn(async move {
            while let Some(mail) = inbox.recv().await {
                match mail {
                    PlatformMail::DeployCanary(deploy_params) => {
                        let outbox = deploy_params.outbox;
                        let result = platform.deploy().await;
                        outbox.send(result).unwrap();
                    }
                    PlatformMail::RollbackCanary(rollback_params) => {
                        let outbox = rollback_params.outbox;
                        let result = platform.rollback_canary().await;
                        outbox.send(result).unwrap();
                    }
                    PlatformMail::PromoteCanary(promote_params) => {
                        let outbox = promote_params.outbox;
                        let result = platform.rollback_canary().await;
                        outbox.send(result).unwrap();
                    }
                }
            }
        });
        let obj_handle = PlatformHandle::new(Arc::new(outbox));
        Self {
            artifact,
            task_done: join_handle,
            handle: obj_handle,
        }
    }

    pub fn handle(&self) -> PlatformHandle {
        self.handle.clone()
    }
}

enum PlatformMail {
    DeployCanary(DeployParams),
    RollbackCanary(RollbackParams),
    PromoteCanary(PromoteParams),
}

struct DeployParams {
    /// The sender where the response is written.
    outbox: oneshot::Sender<DeployResp>,
    // TODO: The params to Deploy go here.
}

impl DeployParams {
    fn new(outbox: oneshot::Sender<DeployResp>) -> Self {
        Self { outbox }
    }
}

struct RollbackParams {
    /// The sender where the response is written.
    outbox: oneshot::Sender<RollbackResp>,
    // TODO: The params to Deploy go here.
}

impl RollbackParams {
    fn new(outbox: oneshot::Sender<RollbackResp>) -> Self {
        Self { outbox }
    }
}

struct PromoteParams {
    /// The sender where the response is written.
    outbox: oneshot::Sender<PromoteResp>,
    // TODO: The params to Deploy go here.
}

impl PromoteParams {
    fn new(outbox: oneshot::Sender<PromoteResp>) -> Self {
        Self { outbox }
    }
}

type DeployResp = Result<()>;
type RollbackResp = Result<()>;
type PromoteResp = Result<()>;

#[async_trait]
impl IntoSubsystem<Report> for PlatformSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        subsys.on_shutdown_requested().await;
        // TODO: Do we need a second channel to shutdown with priority?
        // The channel will be shutdown when all copies of the sender
        // are dropped.
        Ok(())
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
