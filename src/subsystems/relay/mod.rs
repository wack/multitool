use async_trait::async_trait;
use bon::bon;
use chrono::{DateTime, Utc};
use miette::{IntoDiagnostic, Report, Result, bail};
use multitool_sdk::models::DeploymentState;
use multitool_sdk::models::DeploymentStateType::{
    DeployCanary, PromoteCanary, RollbackCanary, SetCanaryTraffic,
};
use tokio::time::Duration;
use tokio::{select, sync::mpsc::Receiver};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle};

use crate::WholePercent;
use crate::adapters::LockedState;
use crate::{
    adapters::{BackendClient, BoxedIngress, BoxedPlatform, DeploymentMetadata, StatusCode},
    stats::Observation,
};

pub const RELAY_SUBSYSTEM_NAME: &str = "relay";

use lock_mgmt::LockManager;
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
        let mut state_stream = match poller.take_stream() {
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
                // • We also need to poll the backend for new states.
                elem = state_stream.recv() => {
                    if let Some(state) = elem {
                        // When we receive a new state, we attempt to lock it.
                        let mut lock_manager = LockManager::builder()
                            .backend(self.backend.clone())
                            .metadata(self.meta.clone())
                            .state(state.clone())
                            .build().await?;
                        let locked_state = lock_manager.state().clone();
                        // The done_callback tells the LockManager not to bother
                        // refreshing the lock, and to tell the backend that
                        // the state has been completed.
                        let done_callback = if let Some(done) = lock_manager.take() {
                           done
                        } else {
                           bail!(
                                "Unreachable. Internal state corrupted on the relay subsystem. Please report this as a bug."
                            );
                        };
                        // Launch the lock manager.
                        subsys.start(SubsystemBuilder::new(
                            format!("LockManager {}", state.id),
                            lock_manager.into_subsystem(),
                        ));
                        // Now that we have the lock managed, we
                        // need to tell the Platform/Ingress
                        // to effect the state.
                        match locked_state.state().state_type {
                            PromoteCanary => {
                                // Ingress operation.
                                self.ingress.promote_canary().await?;
                            },
                            DeployCanary => {
                                // First, we deploy the canary to the platform. At
                                // this point, it won't have any traffic, and the ingress doesn't
                                // know anything about it.
                                self.platform.deploy().await?;
                                // Next, we need the ingress to acknowledge the platform's existance,
                                // creating a CanarySettings objects with zero traffic.
                                // TODO: We also will need a deploy hook for the Ingress.
                                todo!();
                            },
                            SetCanaryTraffic => {
                                 // TODO: Capture percentage from data field of the DeploymentState object.
                                let percent = WholePercent::try_from(0).unwrap();
                                self.ingress.set_canary_traffic(percent).await?;
                                todo!();
                            },
                            RollbackCanary => {
                                // Set traffic to 0 immediately.
                                self.ingress.set_canary_traffic(WholePercent::try_from(0).unwrap()).await?;
                                // Then, yank the canary from the ingress.
                                self.ingress.rollback_canary().await?;
                                // TODO: We don't currently support yanking from the platform for Lambda.
                                // Finally, yank the canary from the platform. [Optional]
                            },
                        }
                        // Since the action completed successfully,
                        // we can release the lock and tell the backend
                        // that the state has been effected.
                        done_callback.send(()).await.into_diagnostic()?;
                    } else {
                        // The stream has been closed, so we should shutdown.
                        subsys.request_shutdown();
                    }
                }
            }
        }
    }
}

// /// HACK: This code calculates the duration of the lock
// /// by comparing the current time to the expiry. This isn't super
// /// smart because it doesn't account for latency. The backend
// /// should tell us how long locks are valid for.
// fn lock_duration(state: &DeploymentState) -> Duration {
//     let now = Utc::now();
//     let expiry = state.expiry();
// }

// mod lease_mgmt;
mod lock_mgmt;
mod poll_state;
// mod renewer;
