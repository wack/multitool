use async_trait::async_trait;
use tokio::{select, sync::mpsc::{self, Receiver, Sender}};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use miette::{Report, Result};
use tokio::time::{Interval, Duration, interval};

use crate::{adapters::{BackendClient, DeploymentMetadata, StateId}, subsystems::ShutdownResult, Shutdownable};

pub(super) struct LeaseRenewer {
    meta: DeploymentMetadata,
    state_id: StateId,
    timer: Interval,
    backend: BackendClient,
    task_done: Receiver<()>,
    /// We give this to the caller to signal when they're
    /// done with the task.
    done_sender: Option<Sender<()>>,
}

impl LeaseRenewer {
    pub(super) fn new(meta: DeploymentMetadata, state_id: StateId, backend: BackendClient, period: Duration) -> Self {
        let (done_sender, task_done) = mpsc::channel(1);
        let timer = interval(period);
        Self {
            meta, state_id, backend,
            timer,
            task_done,
            done_sender: Some(done_sender),
        }
    }
    
}

impl LeaseRenewer {
    pub fn take(&mut self) -> Option<Sender<(())>> {
        self.done_sender.take()
    }
}

#[async_trait]
impl IntoSubsystem<Report> for LeaseRenewer {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        loop {
            select! {
                // Wait for shutdown.
                _ = subsys.on_shutdown_requested() => {
                    // Release the lease.
                    return self.shutdown().await;
                }
                _ = self.task_done.recv() => {
                    // Tell the backend that the task
                    // has been completed.
                    // Don't call `shutdown` since that's for abnormal
                    // termination in this case. We don't need to release
                    // the lock on the state, since we just marked it as completed
                    // instead.
                    return self.backend.mark_state_completed(&self.meta, self.state_id).await;
                } 
                // Ding! Renew the lease.
                _ = self.timer.tick() => {
                    self.backend.refresh_lease(&self.meta, self.state_id).await?;
                }
            }
        }
    }
}

#[async_trait]
impl Shutdownable for LeaseRenewer {
    /// Called when we've received the shutdown signal, but
    /// haven't finished our work yet. We must release the lock,
    /// even though we havne't completed the task.
    async fn shutdown(&mut self) -> ShutdownResult {
        self.backend.abandon_state(&self.meta, self.state_id).await
    }
}
