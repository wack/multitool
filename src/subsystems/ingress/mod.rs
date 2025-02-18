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

use super::ShutdownResult;

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
    shutdown_trigger: Sender<()>,
    task_done: JoinHandle<ShutdownResult>,
    outbox: Sender<IngressMail>,
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
                            todo!("This is where the code goes that matches on mail.")
                        } else {
                            return ingress.shutdown().await;
                        }
                    }
                }
            }
        });
        Self {
            outbox: mail_outbox,
            shutdown_trigger,
            task_done,
        }
    }
}

#[async_trait]
impl IntoSubsystem<Report> for IngressSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        subsys.on_shutdown_requested().await;
        // Send the shutdown signal.
        self.shutdown_trigger
            .send(())
            .await
            .map_err(|err| miette!("could not send shutdown signal: {err}"))?;
        // Wait for the thread to be shutdown.
        self.task_done.await.into_diagnostic()?
    }
}

// The IngressSubsystem handles synchronizing access to the
// `BoxedIngress` using message-passing and channels.
//pub struct IngressSubsystem {
// This is where we write messages for the `[BoxedIngress]` to receive.
// handle: IngressHandle,
// thread_done: JoinHandle<ShutdownResult>,

/*
impl IngressSubsystem {
    pub fn handle(&self) -> IngressHandle {
        self.handle.clone()
    }

    pub async fn new(ingress: BoxedIngress) -> Self {
        // • Spawn a new task with the BoxedIngress and the mailbox.
        let (outbox, inbox) = mpsc::channel(INGRESS_MAILBOX_SIZE);
        let (shutdown_trigger, shutdown_signal) = oneshot::channel();
        // • Spawn the thread that reads from the mailbox and processes
        //   each request.
        let thread_done = IngressRunner::builder()
            .ingress(ingress)
            .inbox(inbox)
            .build()
            .start()
            .await;
        // Return the Subsystem, storing the outbox and the join handle.
        let obj_handle = IngressHandle::new(Arc::new(outbox), shutdown_trigger);
        Self {
            thread_done,
            handle: obj_handle,
        }
    }
}

impl IngressSubsystem {
    pub async fn set_canary_traffic(&mut self, percent: CanaryTrafficPercent) -> Result<()> {
        self.handle.set_canary_traffic(percent).await
    }
}

#[async_trait]
impl IntoSubsystem<Report> for IngressSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        subsys.on_shutdown_requested().await;
        // Send the shutdown signal.
        self.shutdown_trigger
            .send(())
            .map_err(|err| miette!("could not send shutdown signal: {err}"))?;
        // Wait for the thread to be shutdown.
        self.thread_done.await.into_diagnostic()?
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
*/
