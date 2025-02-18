use std::sync::Arc;

use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot,
};

use crate::adapters::Ingress;
use crate::subsystems::ShutdownResult;
use crate::Shutdownable;

use super::{
    mail::{IngressMail, TrafficParams},
    CanaryTrafficPercent,
};

#[derive(Clone)]
pub struct IngressHandle {
    outbox: Arc<Sender<IngressMail>>,
    shutdown_trigger: mpsc::Sender<()>,
}

#[async_trait]
impl Ingress for IngressHandle {
    async fn set_canary_traffic(&mut self, percent: CanaryTrafficPercent) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = TrafficParams::new(sender, percent);
        let mail = IngressMail::SetCanaryTraffic(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
}

#[async_trait]
impl Shutdownable for IngressHandle {
    async fn shutdown(&mut self) -> ShutdownResult {
        self.shutdown_trigger.send(()).await;
        todo!();
    }
}

impl IngressHandle {
    pub(super) fn new(
        outbox: Arc<Sender<IngressMail>>,
        shutdown_trigger: mpsc::Sender<()>,
    ) -> Self {
        Self {
            outbox,
            shutdown_trigger,
        }
    }
}
