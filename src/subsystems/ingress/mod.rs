use std::sync::Arc;

use async_trait::async_trait;
use mail::IngressMail;
use miette::{IntoDiagnostic, Report, Result};
use tokio::select;
use tokio::sync::mpsc::channel;
use tokio::task::JoinHandle;
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::adapters::BoxedIngress;

use super::{ShutdownResult, Shutdownable};

use mail::IngressHandle;

mod mail;

pub const INGRESS_SUBSYSTEM_NAME: &str = "ingress";
/// If you're going to pick an arbitrary number, you could do worse
/// than picking a power of two.
const INGRESS_MAILBOX_SIZE: usize = 1 << 4;

pub struct IngressSubsystem {
    task_done: JoinHandle<ShutdownResult>,
    handle: IngressHandle,
}

impl IngressSubsystem {
    pub fn new(mut ingress: BoxedIngress) -> Self {
        let (shutdown_trigger, mut shutdown_signal) = channel(1);
        let (mail_outbox, mut mail_inbox) = channel(INGRESS_MAILBOX_SIZE);
        let task_done = tokio::spawn(async move {
            loop {
                select! {
                    _ = shutdown_signal.recv() => {
                        return ingress.shutdown().await;
                    }
                    mail = mail_inbox.recv() => {
                        if let Some(mail) = mail {
                            match mail {
                                IngressMail::SetCanaryTraffic(params) => {
                                    let percent = params.percent;
                                    let result = ingress.set_canary_traffic(percent).await;
                                    params.outbox.send(result).unwrap();
                                }
                            }
                        } else {
                            return ingress.shutdown().await;
                        }
                    }
                }
            }
        });
        let shutdown = Arc::new(shutdown_trigger);
        let handle = IngressHandle::new(Arc::new(mail_outbox), shutdown);
        Self { handle, task_done }
    }

    pub fn handle(&self) -> IngressHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl IntoSubsystem<Report> for IngressSubsystem {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        subsys.on_shutdown_requested().await;
        // Propagate the shutdown signal.
        let shutdown_result = self.handle.shutdown().await;
        // Wait for the thread to be shutdown.
        let task_result = self.task_done.await.into_diagnostic();
        shutdown_result.and(task_result?)
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
