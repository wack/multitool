use async_trait::async_trait;
use miette::IntoDiagnostic as _;
use tokio::sync::mpsc::Sender;

use std::sync::Arc;

use super::{ShutdownResult, Shutdownable};

/// A handle to a thread, communicating over a channel.
/// The type `M` can be specialized to implement communication
/// with different subsystems.
pub(super) struct Handle<M> {
    pub(super) outbox: Arc<Sender<M>>,
    pub(super) shutdown_trigger: Arc<Sender<()>>,
}

impl<M> Clone for Handle<M> {
    fn clone(&self) -> Self {
        Self {
            outbox: self.outbox.clone(),
            shutdown_trigger: self.shutdown_trigger.clone(),
        }
    }
}

impl<M> Handle<M> {
    pub(super) fn new(outbox: Arc<Sender<M>>, shutdown_trigger: Arc<Sender<()>>) -> Self {
        Self {
            outbox,
            shutdown_trigger,
        }
    }
}

#[async_trait]
impl<M: Send> Shutdownable for Handle<M> {
    async fn shutdown(&mut self) -> ShutdownResult {
        self.shutdown_trigger.send(()).await.into_diagnostic()
    }
}
