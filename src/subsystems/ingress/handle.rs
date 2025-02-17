use std::sync::Arc;

use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};
use tokio::sync::{mpsc::Sender, oneshot};

use crate::adapters::Ingress;

use super::{mail::IngressMail, CanaryTrafficPercent, TrafficParams};

#[derive(Clone)]
pub struct IngressHandle {
    outbox: Arc<Sender<IngressMail>>,
}

#[async_trait]
impl Ingress for IngressHandle {
    async fn set_canary_traffic(&mut self, percent: CanaryTrafficPercent) -> Result<()> {
        let (sender, receiver): (oneshot::Sender<Result<()>>, oneshot::Receiver<Result<()>>) =
            oneshot::channel();
        let params = TrafficParams::new(sender, percent);
        let mail = IngressMail::SetCanaryTraffic(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
}

impl IngressHandle {
    pub(super) fn new(outbox: Arc<Sender<IngressMail>>) -> Self {
        Self { outbox }
    }
}
