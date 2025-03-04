use std::sync::Arc;

use async_trait::async_trait;
use bon::bon;
use miette::{Report, Result, bail};
use tokio::{select, sync::mpsc::Receiver, time::Duration};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle};

use crate::{
    adapters::{BackendClient, BoxedIngress, BoxedPlatform, DeploymentMetadata, StatusCode},
    stats::Observation,
};

pub const RELAY_SUBSYSTEM_NAME: &str = "relay";

use lock_mgmt::LockManagementSubsystem;
use poll_state::StatePoller;

/// The RelaySubsystem is responsible for sending messages
/// to and from the backend.
pub struct RelaySubsystem<T: Observation + Send + 'static> {
    /// The relay subsystem needs a backend client
    /// so it can send monitoring data to the backend,
    /// update the backend when a new state is effected,
    /// and poll for new states to apply.
    backend: BackendClient,
    // These observations come from the MonitorSubsystem.
    // They must be sent to the backend whenever available.
    // Pin<Box<Stream<Item=T: Observation>>
    // NB: This should probably happen in its own thread.
    observations: Receiver<Vec<T>>,
    /// This field provides context about the current deployment,
    /// and is frequently serialized and passed to the backend on
    /// each request.
    meta: DeploymentMetadata,
    // TODO:
    // Every so often, we need to poll the backend for
    // instructions. These instructions come in the form
    // of state.
    //
    // Each time we get new state, we need to lock the state,
    // manage the locks, and make a request to the Ingress
    // or Platform to effect the state.
    platform: BoxedPlatform,
    ingress: BoxedIngress,
    backend_poll_frequency: Option<Duration>,
}

#[bon]
impl<T: Observation + Send + 'static> RelaySubsystem<T> {
    #[builder]
    pub fn new(
        backend: BackendClient,
        meta: DeploymentMetadata,
        observations: Receiver<Vec<T>>,
        platform: BoxedPlatform,
        ingress: BoxedIngress,
        backend_poll_frequency: Option<Duration>,
    ) -> Self {
        Self {
            backend,
            meta,
            observations,
            platform,
            ingress,
            backend_poll_frequency,
        }
    }

    fn new_poller(&mut self) -> StatePoller {
        let builder = StatePoller::builder()
            .meta(self.meta.clone())
            .backend(self.backend.clone());
        if let Some(freq) = self.backend_poll_frequency {
            builder.freq(freq).build()
        } else {
            builder.build()
        }
    }
}

#[async_trait]
impl IntoSubsystem<Report> for RelaySubsystem<StatusCode> {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        // Kick off a task to poll the backend for new states.
        let mut poller = self.new_poller();
        let state_stream = match poller.take_stream() {
            None => bail!(
                "Unreachable. Internal state corrupted on the relay subsystem. Please report this as a bug."
            ),
            Some(stream) => stream,
        };

        subsys.start(SubsystemBuilder::new(
            "StatePoller",
            poller.into_subsystem(),
        ));

        let mut observations = self.observations;
        loop {
            select! {
                // TODO: We need to release the lock on
                // any states we've locked.
                // Besides that, we can just hang out.
                _ = subsys.on_shutdown_requested() => {
                    subsys.wait_for_children().await;
                    return Ok(());
                }
                // • When we start the RelaySubsystem,
                //   we need to select on the observation stream.
                //   When a new observation arrives, we send it to the backend.
                elem = observations.recv() => {
                    if let Some(batch) = elem {
                        self.backend.upload_observations(&self.meta, batch).await?;
                    } else {
                        // The stream has been closed, so we should shutdown.
                        subsys.request_shutdown();
                    }
                }
            }
        }
        // • We also need to poll the backend for new states.
        //   Each new state results in a sequence of calls to the Platform
        //   and Ingress. Once those complete, we send an update to the backend.
    }
}

mod lease_mgmt;
mod lock_mgmt;
mod poll_state;
mod renewer;
