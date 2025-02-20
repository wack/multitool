use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};
use tokio::sync::oneshot;

use crate::{WholePercent, adapters::Ingress, subsystems::handle::Handle};

pub(super) type IngressHandle = Handle<IngressMail>;

pub(super) enum IngressMail {
    SetCanaryTraffic(TrafficParams),
    RollbackCanary(RollbackParams),
    PromoteCanary(PromoteParams),
}

#[async_trait]
impl Ingress for IngressHandle {
    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = TrafficParams::new(sender, percent);
        let mail = IngressMail::SetCanaryTraffic(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
    async fn rollback_canary(&mut self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = RollbackParams::new(sender);
        let mail = IngressMail::RollbackCanary(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }

    async fn promote_canary(&mut self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = PromoteParams::new(sender);
        let mail = IngressMail::PromoteCanary(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
}

pub(super) struct TrafficParams {
    /// The sender where the response is written.
    pub(super) outbox: oneshot::Sender<TrafficResp>,
    /// The amount of traffic the user is expected to receive.
    pub(super) percent: WholePercent,
}

impl TrafficParams {
    pub(super) fn new(outbox: oneshot::Sender<TrafficResp>, percent: WholePercent) -> Self {
        Self { outbox, percent }
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
pub(super) type TrafficResp = Result<()>;
