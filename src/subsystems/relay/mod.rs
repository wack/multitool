use std::sync::Arc;

use async_trait::async_trait;
use bon::bon;
use miette::{Report, Result};
use tokio::{pin, select, sync::mpsc::Receiver};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::{
    adapters::{BackendClient, BoxedIngress, BoxedPlatform},
    stats::Observation,
};

pub const RELAY_SUBSYSTEM_NAME: &str = "relay";

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
    // Every so often, we need to poll the backend for
    // instructions. These instructions come in the form
    // of state.
    //
    // Each time we get new state, we need to lock the state,
    // manage the locks, and make a request to the Ingress
    // or Platform to effect the state.
    platform: BoxedPlatform,
    ingress: BoxedIngress,
}

#[bon]
impl<T: Observation + Send + 'static> RelaySubsystem<T> {
    #[builder]
    pub fn new(
        backend: BackendClient,
        observations: Receiver<Vec<T>>,
        platform: BoxedPlatform,
        ingress: BoxedIngress,
    ) -> Self {
        Self {
            backend,
            observations,
            platform,
            ingress,
        }
    }
}

#[async_trait]
impl<T: Observation + Send + Sync> IntoSubsystem<Report> for RelaySubsystem<T> {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        let observations = self.observations;
        pin!(observations);
        loop {
            select! {
                // TODO: We need to release the lock on
                // any states we've locked.
                // Besides that, we can just hang out.
                _ = subsys.on_shutdown_requested() => {
                    return Ok(());
                }
                // • When we start the RelaySubsystem,
                //   we need to select on the observation stream.
                //   When a new observation arrives, we send it to the backend.
                elem = observations.recv() => {
                    if let Some(batch) = elem {
                        // self.backend.upload_observations(batch).await?;
                        self.backend.upload_observations(vec![]).await?;
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
mod renewer;
