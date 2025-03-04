use async_trait::async_trait;
use bon::bon;
use miette::{IntoDiagnostic, Report, Result};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::{
    Shutdownable,
    adapters::{BackendClient, DeploymentMetadata},
    subsystems::ShutdownResult,
};
use multitool_sdk::models::DeploymentState;
use tokio::{
    select,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Interval, interval},
};

/// This is the amount of time between calls to the backend to
/// refresh the list of states that need to be applied.
const DEFAULT_POLLING_FREQUENCY: Duration = Duration::from_secs(10);
/// If you're going to pick an arbitrary number, you could do worse
/// than picking a power of two.
const DEFAULT_CHANNEL_SIZE: usize = 1 << 5;

pub struct StatePoller {
    /// This is the client we use to poll for new state.
    backend: BackendClient,
    /// This timer ticks every so often, letting us know
    /// its time to poll the backend for new state.
    timer: Interval,
    /// This field describes the current active deployment. It's
    /// context we pass to the backend on each request.
    meta: DeploymentMetadata,
    /// This is where we write new messages when we have them.
    outbox: Sender<DeploymentState>,
    /// We give this to the caller so it can stream new
    /// messages.
    stream: Option<Receiver<DeploymentState>>,
}

#[bon]
impl StatePoller {
    #[builder]
    pub(super) fn new(
        meta: DeploymentMetadata,
        backend: BackendClient,
        freq: Option<Duration>,
    ) -> Self {
        let freq = freq.unwrap_or(DEFAULT_POLLING_FREQUENCY);
        let timer = interval(freq);
        let (outbox, inbox) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        Self {
            backend,
            meta,
            timer,
            outbox,
            stream: Some(inbox),
        }
    }

    pub fn take_stream(&mut self) -> Option<Receiver<DeploymentState>> {
        self.stream.take()
    }
}

#[async_trait]
impl IntoSubsystem<Report> for StatePoller {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        // Periodically poll the backend for updates.
        loop {
            select! {
                _ = subsys.on_shutdown_requested() => {
                    return self.shutdown().await
                }
                _ = self.timer.tick() => {
                    // Poll the backend for new states, then
                    // pass them off over the channel.
                    let states = self.backend.poll_for_state(&self.meta).await?;
                    for state in states {
                        self.outbox.send(state).await.into_diagnostic()?;
                    }
                }
            }
        }
    }
}

#[async_trait]
impl Shutdownable for StatePoller {
    async fn shutdown(&mut self) -> ShutdownResult {
        // Nothing to do! We just stop polling.
        Ok(())
    }
}
