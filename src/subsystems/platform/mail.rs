use miette::Result;
use tokio::sync::oneshot;

pub(super) enum PlatformMail {
    DeployCanary(DeployParams),
    RollbackCanary(RollbackParams),
    PromoteCanary(PromoteParams),
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

pub(super) struct RollbackParams {
    /// The sender where the response is written.
    pub(super) outbox: oneshot::Sender<RollbackResp>,
    // TODO: The params to Deploy go here.
}

impl RollbackParams {
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
