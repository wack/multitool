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

    async fn set_baseline_version_id(&mut self, version_id: String) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = VersionParams::new(sender, version_id);
        let mail = MonitorMail::SetBaselineVersionId(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }

    async fn set_canary_version_id(&mut self, version_id: String) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        let params = VersionParams::new(sender, version_id);
        let mail = MonitorMail::SetCanaryVersionId(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
}

pub(super) enum MonitorMail<T: Observation> {
    Query(QueryParams<T>),
    SetBaselineVersionId(VersionParams),
    SetCanaryVersionId(VersionParams),
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

pub(super) struct VersionParams {
    /// The sender where the response is written.
    pub(super) outbox: oneshot::Sender<VersionResp>,
    pub(super) version_id: String,
}

impl VersionParams {
    pub(super) fn new(outbox: oneshot::Sender<VersionResp>, version_id: String) -> Self {
        Self { outbox, version_id }
    }
}

pub(super) type QueryResp<T> = Result<Vec<T>>;

pub(super) type VersionResp = Result<()>;
