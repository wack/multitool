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
    shutdown_trigger: Arc<mpsc::Sender<()>>,
}

impl IngressHandle {
    pub(super) fn new(
        outbox: Arc<Sender<IngressMail>>,
        shutdown_trigger: Arc<mpsc::Sender<()>>,
    ) -> Self {
        Self {
            outbox,
            shutdown_trigger,
        }
    }
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

/// Send the shutdown signal. The `IngressHandle` does not wait
/// for shutdown, it only propagates the signal. This is because
/// other top-level subsystems wait for the shutdown signal,
/// and the joinhandle is not clone.
#[async_trait]
impl Shutdownable for IngressHandle {
    async fn shutdown(&mut self) -> ShutdownResult {
        self.shutdown_trigger.send(()).await.into_diagnostic()
    }
}

#[cfg(test)]
mod tests {
    use super::IngressHandle;
    use crate::adapters::Ingress;

    use static_assertions::assert_impl_all;

    assert_impl_all!(IngressHandle: Ingress);
}
