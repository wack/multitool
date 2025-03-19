use std::sync::Arc;

use async_trait::async_trait;
use mail::{IngressMail, PromoteParams, ReleaseParams, RollbackParams, TrafficParams};
use miette::{Report, Result};
use tokio::sync::mpsc::channel;
use tokio::{select, sync::mpsc::Receiver};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::adapters::BoxedIngress;

use mail::IngressHandle;

use super::{ShutdownResult, Shutdownable};

mod mail;

pub const INGRESS_SUBSYSTEM_NAME: &str = "ingress";
/// If you're going to pick an arbitrary number, you could do worse
/// than picking a power of two.
const INGRESS_MAILBOX_SIZE: usize = 1 << 4;

pub struct IngressSubsystem {
    ingress: BoxedIngress,
    handle: IngressHandle,
    mailbox: Receiver<IngressMail>,
    shutdown: Receiver<()>,
}

impl IngressSubsystem {
    pub fn new(ingress: BoxedIngress) -> Self {
        let (shutdown_trigger, shutdown_signal) = channel(1);
        let (mail_outbox, mailbox) = channel(INGRESS_MAILBOX_SIZE);
        let shutdown = Arc::new(shutdown_trigger);
        let handle = IngressHandle::new(Arc::new(mail_outbox), shutdown);
        Self {
            handle,
            ingress,
            mailbox,
            shutdown: shutdown_signal,
        }
    }

    /// Create a new handle to the underlying ingress. The handle is a BoxedIngress itself,
    /// but it communicates with the real ingress over a channel, so it's Send+Sync+Clone.
    pub fn handle(&self) -> BoxedIngress {
        Box::new(self.handle.clone())
    }

    async fn respond_to_mail(&mut self, mail: IngressMail) {
        match mail {
            IngressMail::Release(params) => self.handle_release(params).await,
            IngressMail::RollbackCanary(params) => self.handle_rollback(params).await,
            IngressMail::PromoteCanary(params) => self.handle_promote(params).await,
            IngressMail::SetCanaryTraffic(params) => self.handle_set_traffic(params).await,
        }
    }

    async fn handle_release(&mut self, params: ReleaseParams) {
        let result = self.ingress.release_canary(params.platform_id).await;
        params.outbox.send(result).unwrap();
    }

    async fn handle_rollback(&mut self, params: RollbackParams) {
        let result = self.ingress.rollback_canary().await;
        params.outbox.send(result).unwrap();
    }

    async fn handle_promote(&mut self, params: PromoteParams) {
        let result = self.ingress.promote_canary().await;
        params.outbox.send(result).unwrap();
    }

    async fn handle_set_traffic(&mut self, params: TrafficParams) {
        let percent = params.percent;
        let result = self.ingress.set_canary_traffic(percent).await;
        params.outbox.send(result).unwrap();
    }
}

#[async_trait]
impl IntoSubsystem<Report> for IngressSubsystem {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        loop {
            select! {
                _ = subsys.on_shutdown_requested() => {
                    return self.shutdown().await;
                }
                // Shutdown signal from one of the handles. Since this thread has exclusive
                // access to the platform, we have to give the outside world a way to shut
                // us down. That's this channel, created before the SubsystemHandle existed.
                _ = self.shutdown.recv() => {
                    subsys.request_shutdown();
                }
                mail = self.mailbox.recv() => {
                    if let Some(mail) = mail {
                        self.respond_to_mail(mail).await;
                    } else {
                        dbg!("Stream closed in ingress");
                        return self.shutdown().await;
                    }
                }
            }
        }
    }
}

#[async_trait]
impl Shutdownable for IngressSubsystem {
    async fn shutdown(&mut self) -> ShutdownResult {
        // We just have to shut the ingress down manually,
        // since we have an exclusive lock on it.
        self.ingress.shutdown().await
    }
}

#[cfg(test)]
mod tests {
    use super::IngressSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(IngressSubsystem: IntoSubsystem<Report>);
}
