use async_trait::async_trait;
use miette::{IntoDiagnostic as _, Result};
use tokio::sync::oneshot;

use crate::{adapters::Monitor, stats::Observation, subsystems::handle::Handle};

pub(super) type MonitorHandle<T> = Handle<MonitorMail<T>>;

#[async_trait]
impl<T: Observation + Send + 'static> Monitor for MonitorHandle<T> {
    type Item = T;
    async fn query(&mut self) -> Result<Vec<T>> {
        let (sender, receiver) = oneshot::channel();
        let params = QueryParams::new(sender);
        let mail = MonitorMail::Query(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
}

pub(super) enum MonitorMail<T: Observation> {
    Query(QueryParams<T>),
}

pub(super) struct QueryParams<T: Observation> {
    /// The sender where the response is written.
    pub(super) outbox: oneshot::Sender<QueryResp<T>>,
}

impl<T: Observation> QueryParams<T> {
    pub(super) fn new(outbox: oneshot::Sender<QueryResp<T>>) -> Self {
        Self { outbox }
    }
}

pub(super) type QueryResp<T> = Result<Vec<T>>;
