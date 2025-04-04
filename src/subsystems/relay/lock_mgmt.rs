use async_trait::async_trait;
use bon::bon;
use miette::miette;
use miette::{Report, Result};
use multitool_sdk::models::RolloutState;
use tokio::select;
use tokio::sync::mpsc::{self, Receiver};
use tokio::sync::oneshot;
use tokio::time::{Interval, interval};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::{
    Shutdownable,
    adapters::{BackendClient, RolloutMetadata},
    subsystems::ShutdownResult,
};

use super::LockedState;

pub(super) struct LockManager {
    /// We use this client to refresh locks.
    backend: BackendClient,
    /// This field describes the current active rollout.
    /// This is context we pass to the backend on each request.
    meta: RolloutMetadata,
    /// This is the state that this manager is locking.
    state: LockedState,
    /// This timer ticks every time we should refresh the lock.
    timer: Interval,
    /// This channls is filled when the state has been effected
    /// by the ingress/platform. It signals to us to let the
    /// backend know the state has been achieved, and we can
    /// shutdown.
    task_done: Receiver<oneshot::Sender<()>>,
}

#[bon]
impl LockManager {
    #[builder]
    pub(super) async fn new(
        backend: BackendClient,
        metadata: RolloutMetadata,
        state: RolloutState,
    ) -> Result<Self> {
        let (done_sender, task_done) = mpsc::channel(1);
        // Take the initial lock.
        let locked_state = backend.lock_state(&metadata, &state, done_sender).await?;
        let freq = *locked_state.frequency();
        let timer = interval(freq / 2);
        Ok(Self {
            backend,
            state: locked_state,
            timer,
            task_done,
            meta: metadata,
        })
    }

    pub(super) fn state(&self) -> &LockedState {
        &self.state
    }
}

#[async_trait]
impl IntoSubsystem<Report> for LockManager {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        loop {
            select! {
                // NOTE: the order matters here
                done = self.task_done.recv() => {
                    if let Some(responder) = done {
                        // Tell the backend that the task has been completed.
                        // Don't call `shutdown` since that's for abnormal
                        // termination in this case. We don't need to release
                        // the lock on the state, since we just marked it as completed
                        // instead.
                        self.backend.mark_state_completed(&self.meta, &self.state).await?;
                        return responder.send(()).map_err(|_|miette!("Channel closed before completing"));
                    }
                 }
                _ = subsys.on_shutdown_requested() => {
                    // Release the lock.
                    return self.shutdown().await;
                }
                // Ding! Renew the lease.
                _ = self.timer.tick() => {
                    self.backend.refresh_lock(&self.meta, &self.state).await?;
                }
            }
        }
    }
}

#[async_trait]
impl Shutdownable for LockManager {
    async fn shutdown(&mut self) -> ShutdownResult {
        // Release any of the locks we've taken.
        self.backend.abandon_lock(&self.meta, &self.state).await
    }
}
