use std::sync::Arc;

use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};
use tokio::sync::{mpsc::Sender, oneshot};

use crate::{adapters::Monitor, stats::Observation};

use super::mail::{MonitorMail, QueryParams};

#[derive(Clone)]
pub struct MonitorHandle<T: Observation + Clone> {
    outbox: Arc<Sender<MonitorMail<T>>>,
}

#[async_trait]
impl<T: Observation + Clone + Send + 'static> Monitor for MonitorHandle<T> {
    type Item = T;
    async fn query(&mut self) -> Result<Vec<T>> {
        let (sender, receiver) = oneshot::channel();
        let params = QueryParams::new(sender);
        let mail = MonitorMail::Query(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
}

impl<T: Observation + Clone> MonitorHandle<T> {
    pub(super) fn new(outbox: Arc<Sender<MonitorMail<T>>>) -> Self {
        Self { outbox }
    }
}
