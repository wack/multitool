use async_trait::async_trait;
use miette::{IntoDiagnostic as _, Result};
use tokio::sync::oneshot;

use crate::{adapters::Platform, subsystems::handle::Handle};

pub(super) type PlatformHandle = Handle<PlatformMail>;

pub(super) enum PlatformMail {
    DeployCanary(DeployParams),
    YankCanary(YankParams),
    PromoteDeployment(PromoteParams),
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

    async fn yank_canary(&mut self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = YankParams::new(sender);
        let mail = PlatformMail::YankCanary(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }

    async fn promote_deployment(&mut self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = PromoteParams::new(sender);
        let mail = PlatformMail::PromoteDeployment(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
}

pub(super) struct DeployParams {
    /// The sender where the response is written.
    pub(super) outbox: oneshot::Sender<DeployResp>,
    // TODO: The params to Deploy go here.
}

impl DeployParams {
    pub(super) fn new(outbox: oneshot::Sender<DeployResp>) -> Self {
        Self { outbox }
    }
}

pub(super) struct YankParams {
    /// The sender where the response is written.
    pub(super) outbox: oneshot::Sender<RollbackResp>,
    // TODO: The params to Deploy go here.
}

impl YankParams {
    pub(super) fn new(outbox: oneshot::Sender<RollbackResp>) -> Self {
        Self { outbox }
    }
}

pub(super) struct PromoteParams {
    /// The sender where the response is written.
    pub(super) outbox: oneshot::Sender<PromoteResp>,
    // TODO: The params to Deploy go here.
}

impl PromoteParams {
    pub(super) fn new(outbox: oneshot::Sender<PromoteResp>) -> Self {
        Self { outbox }
    }
}

pub(super) type DeployResp = Result<()>;
pub(super) type RollbackResp = Result<()>;
pub(super) type PromoteResp = Result<()>;
