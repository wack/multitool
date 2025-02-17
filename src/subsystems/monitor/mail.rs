use miette::Result;
use tokio::sync::oneshot;

use crate::stats::Observation;

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

pub(super) type QueryResp<T: Observation> = Result<Vec<T>>;
