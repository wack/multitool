use std::fmt;
use std::sync::Arc;

use crate::adapters::{BoxedMonitor, Monitor};
use crate::stats::Observation;
use async_trait::async_trait;
use mail::MonitorMail;
use miette::{IntoDiagnostic as _, Report, Result};
use tokio::{select, sync::mpsc::channel, task::JoinHandle};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use super::{ShutdownResult, Shutdownable};

use mail::MonitorHandle;

pub const MONITOR_SUBSYSTEM_NAME: &str = "monitor";
/// If you're going to pick an arbitrary number, you could do worse
/// than picking a power of two.
const MONITOR_MAILBOX_SIZE: usize = 1 << 4;

pub struct MonitorSubsystem<T: Observation> {
    task_done: JoinHandle<ShutdownResult>,
    handle: MonitorHandle<T>,
}

impl<T: Observation + fmt::Debug + Send + 'static> MonitorSubsystem<T> {
    pub fn new(mut monitor: BoxedMonitor<T>) -> Self {
        let (shutdown_trigger, mut shutdown_signal) = channel(1);

        let (mail_outbox, mut mail_inbox) = channel(MONITOR_MAILBOX_SIZE);
        let task_done = tokio::spawn(async move {
            loop {
                select! {
                    _ = shutdown_signal.recv() => {
                        return monitor.shutdown().await;
                    }
                    mail = mail_inbox.recv() => {
                        match mail {
                            Some(mail) => {
                                match mail {
                                    MonitorMail::Query(params) => {
                                        let result = monitor.query().await;
                                        params.outbox.send(result).unwrap();
                                    }
                                }
                            },
                            _ => {
                                return monitor.shutdown().await;
                            }
                        }
                    }
                }
            }
        });
        let shutdown = Arc::new(shutdown_trigger);
        let handle = MonitorHandle::new(Arc::new(mail_outbox), shutdown);
        Self { handle, task_done }
    }

    /// Returns a shallow copy of the Monitor, using a channel and a handle.
    pub fn handle(&self) -> BoxedMonitor<T> {
        Box::new(self.handle.clone())
    }
}

#[async_trait]
impl<T: Observation + Send + 'static> IntoSubsystem<Report> for MonitorSubsystem<T> {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        subsys.on_shutdown_requested().await;
        // Propagate the shutdown signal.
        let shutdown_result = self.handle.shutdown().await;
        // Wait for the thread to be shutdown.
        let task_result = self.task_done.await.into_diagnostic();
        shutdown_result.and(task_result?)
    }
}

mod mail;

#[cfg(test)]
mod tests {
    use crate::{metrics::ResponseStatusCode, stats::CategoricalObservation};

    use super::MonitorSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(MonitorSubsystem<CategoricalObservation<5, ResponseStatusCode>>: IntoSubsystem<Report>);
}
