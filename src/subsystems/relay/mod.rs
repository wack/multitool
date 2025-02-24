use std::sync::Arc;

use async_trait::async_trait;
use miette::{Report, Result};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use tokio_stream::Stream;

use crate::{adapters::BackendClient, stats::Observation};

pub const RELAY_SUBSYSTEM_NAME: &str = "relay";

/// The RelaySubsystem is responsible for sending messages
/// to and from the backend.
pub struct RelaySubsystem<T: Observation + 'static> {
    backend: Arc<dyn BackendClient + 'static>,
    // These observations come from the MonitorSubsystem.
    // They must be sent to the backend whenever available.
    // Pin<Box<Stream<Item=T: Observation>>
    // NB: This should probably happen in its own thread.
    observations: Box<dyn Stream<Item = T> + Send + Sync>,
    // Every so often, we need to poll the backend for
    // instructions. These instructions come in the form
    // of state.
    //
    // Each time we get new state, we need to lock the state,
    // manage the locks, and make a request to the Ingress
    // or Platform to effect the state.
}

impl<T: Observation + 'static> RelaySubsystem<T> {
    pub fn new(
        backend: Arc<dyn BackendClient + 'static>,
        observations: Box<dyn Stream<Item = T> + Send + Sync>,
    ) -> Self {
        Self {
            backend,
            observations,
        }
    }
}

#[async_trait]
impl<T: Observation> IntoSubsystem<Report> for RelaySubsystem<T> {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        todo!()
    }
}
