use std::sync::Arc;

use async_trait::async_trait;
use bon::bon;
use miette::{IntoDiagnostic, Report, Result};
use tokio::{
    select,
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, interval},
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use tokio_stream::{StreamExt as _, wrappers::IntervalStream};

use crate::{Shutdownable, adapters::BackendClient, subsystems::ShutdownResult};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(15);

/// The [StatePoller] will poll the backend for new
/// states every so often. It writes new states
/// to a channel for the Relay subsystem to handle.
pub struct StatePoller {
    // TODO: We can get rid of the Arc if we make BackendClient
    // cloneable, and then use the BoxedClone trick to box it.
    client: Arc<dyn BackendClient>,
    sender: Sender<DeploymentState>,
    /// How often we poll the backend for new states.
    poll_interval: Duration,
    recv: Option<Receiver<DeploymentState>>,

    // TODO: Plumb the deployment ID all the way down here.
    deployment_id: u64,
}

#[async_trait]
impl IntoSubsystem<Report> for StatePoller {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        // • Create a timer using our poll interval.
        // • Loop over the timer, selecting on the shutdown
        //   signal and the next tick.
        let mut timer = IntervalStream::new(interval(self.poll_interval));
        loop {
            select! {
                _ = subsys.on_shutdown_requested() => {
                    return self.shutdown().await;
                }
                tick = timer.next() => {
                    // If the timer was shutdown, we should
                    // shutdown, too.
                    if tick.is_none() {
                        return self.shutdown().await;
                    }
                    // Otherwise, poll the backend.
                    // NB: Any error from the backend is fatal. Maybe we should be more
                    // generous to ourselves?
                    let events = self.client.poll_deployment_state(self.deployment_id).await?;
                    for event in events {
                        self.sender.send(event).await.into_diagnostic()?;
                    }
                }
            }
        }
    }
}

#[bon]
impl StatePoller {
    #[builder]
    pub fn new(client: Arc<dyn BackendClient>, poll_interval: Option<Duration>) -> Self {
        let (sender, recv) = mpsc::channel(16);

        Self {
            client,
            sender,
            poll_interval: poll_interval.unwrap_or(DEFAULT_POLL_INTERVAL),
            recv: Some(recv),
            deployment_id: todo!(),
        }
    }

    pub fn take_stream(&mut self) -> Option<Receiver<DeploymentState>> {
        self.recv.take()
    }
}

// TODO: These objects are defined by the OpenAPI spec,
//       but we haven't imported them yet. For now we stub out
//       the type but we will need to actually import them
//       before this will work.
type DeploymentState = ();

#[async_trait]
impl Shutdownable for StatePoller {
    async fn shutdown(&mut self) -> ShutdownResult {
        // Nothing to do at this time.
        // We just stop polling. You can drop
        // use whenever you want.
        Ok(())
    }
}
