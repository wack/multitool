use async_trait::async_trait;
use miette::{IntoDiagnostic as _, Result};
use tokio::sync::oneshot;

use crate::{adapters::Ingress, subsystems::handle::Handle, WholePercent};

pub(super) type IngressHandle = Handle<IngressMail>;

pub(super) enum IngressMail {
    SetCanaryTraffic(TrafficParams),
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

pub(super) type TrafficResp = Result<()>;
