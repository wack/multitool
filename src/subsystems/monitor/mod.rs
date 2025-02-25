use std::fmt;
use std::sync::Arc;

use crate::adapters::BoxedMonitor;
use crate::stats::Observation;
use async_trait::async_trait;
use mail::{MonitorHandle, MonitorMail, QueryParams};
use miette::{Report, Result};
use tokio::{
    select,
    sync::mpsc::{Receiver, channel},
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

pub const MONITOR_SUBSYSTEM_NAME: &str = "monitor";
/// If you're going to pick an arbitrary number, you could do worse
/// than picking a power of two.
const MONITOR_MAILBOX_SIZE: usize = 1 << 4;

pub struct MonitorSubsystem<T: Observation> {
    monitor: BoxedMonitor<T>,
    handle: MonitorHandle<T>,
    mailbox: Receiver<MonitorMail<T>>,
    shutdown: Receiver<()>,
}

impl<T: Observation + fmt::Debug + Send + 'static> MonitorSubsystem<T> {
    pub fn new(monitor: BoxedMonitor<T>) -> Self {
        let (shutdown_trigger, shutdown_signal) = channel(1);
        let (mail_outbox, mailbox) = channel(MONITOR_MAILBOX_SIZE);
        let shutdown = Arc::new(shutdown_trigger);
        let handle = MonitorHandle::new(Arc::new(mail_outbox), shutdown);
        Self {
            monitor,
            handle,
            mailbox,
            shutdown: shutdown_signal,
        }
    }

    /// Returns a shallow copy of the Monitor, using a channel and a handle.
    pub fn handle(&self) -> BoxedMonitor<T> {
        Box::new(self.handle.clone())
    }

    async fn handle_query(&mut self, params: QueryParams<T>) {
        let result = self.monitor.query().await;
        params.outbox.send(result).unwrap();
    }
}

#[async_trait]
impl<T: Observation + Send + 'static> IntoSubsystem<Report> for MonitorSubsystem<T> {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        loop {
            select! {
                _ = subsys.on_shutdown_requested() => {
                    return self.monitor.shutdown().await;
                }
                _ = self.shutdown.recv() => {
                    return self.monitor.shutdown().await;
                }
                mail = self.mailbox.recv() => {
                    if let Some(mail) = mail {
                        match mail {
                            MonitorMail::Query(params) => self.handle_query(params).await,
                        }
                    } else {
                        subsys.request_shutdown();
                    }
                }
            }
        }
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
