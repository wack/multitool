use std::sync::Arc;

use async_trait::async_trait;
use aws_sdk_lambda::config::IntoShared;
use mail::IngressMail;
use miette::miette;
use miette::{IntoDiagnostic, Report, Result};
use tokio::select;
use tokio::sync::mpsc::channel;
use tokio::{
    sync::mpsc::{Receiver, Sender},
    task::JoinHandle,
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::adapters::{BoxedIngress, Ingress};

pub use handle::IngressHandle;

use super::{ShutdownResult, Shutdownable};

mod handle;
mod mail;

pub const INGRESS_SUBSYSTEM_NAME: &str = "ingress";
/// If you're going to pick an arbitrary number, you could do worse
/// than picking a power of two.
const INGRESS_MAILBOX_SIZE: usize = 1 << 4;

/// We anticipate changing this number in the future, so for now we're
/// just going to use a type alias to keep everything localized to one spot.
/// In the prototype, we used a "WholeNumber" percentage to ensure the
/// user only put in a number from 0-100. We can imagine using fractions,
/// but we want to validate that the number is between [0..100] regardless
/// of whether we use fractions or not.
type CanaryTrafficPercent = u32;

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
        // Send the shutdown signal.
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
