use std::ops::ControlFlow;

use bon::bon;
use kameo::actor::{ActorId, ActorRef, Spawn, WeakActorRef};
use kameo::error::{ActorStopReason, Infallible};
use kameo::mailbox;
use kameo::Actor;
use miette::{miette, Result};
use multitool_sdk::models::RolloutStateData;
use multitool_sdk::models::RolloutStateType::{
    CancelCanary, DeployCanary, PromoteCanary, RollbackCanary, SetCanaryTraffic,
};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Duration;
use tracing::{debug, trace};

use crate::adapters::LockedState;
use crate::WholePercent;
use crate::{
    adapters::{BackendClient, BoxedIngress, BoxedPlatform, RolloutMetadata, StatusCode},
    stats::Observation,
};

pub const RELAY_SUBSYSTEM_NAME: &str = "relay";

use lock_mgmt::LockManager;
use poll_state::StatePoller;

/// Arguments for RelaySubsystem initialization.
pub struct RelaySubsystemArgs {
    pub backend: BackendClient,
    pub meta: RolloutMetadata,
    pub observations: Receiver<Vec<StatusCode>>,
    pub platform: BoxedPlatform,
    pub ingress: BoxedIngress,
    pub backend_poll_frequency: Option<Duration>,
    pub baseline_sender: Sender<String>,
    pub canary_sender: Sender<String>,
}

/// The RelaySubsystem is responsible for sending messages
/// to and from the backend.
pub struct RelaySubsystem<T: Observation + Send + 'static> {
    /// The relay subsystem needs a backend client.
    #[allow(dead_code)]
    backend: BackendClient,
    /// Observations from the MonitorSubsystem (stored but taken during on_start).
    #[allow(dead_code)]
    observations: Option<Receiver<Vec<T>>>,
    /// Context about the current rollout.
    #[allow(dead_code)]
    meta: RolloutMetadata,
    /// Platform adapter.
    #[allow(dead_code)]
    platform: BoxedPlatform,
    /// Ingress adapter.
    #[allow(dead_code)]
    ingress: BoxedIngress,
    /// Backend poll frequency.
    #[allow(dead_code)]
    backend_poll_frequency: Option<Duration>,
    /// Sender for baseline version ID.
    #[allow(dead_code)]
    baseline_sender: Sender<String>,
    /// Sender for canary version ID.
    #[allow(dead_code)]
    canary_sender: Sender<String>,
    /// Actor reference to the StatePoller child.
    #[allow(dead_code)]
    poller_ref: Option<ActorRef<StatePoller>>,
    /// Shutdown signal sender.
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Actor for RelaySubsystem<StatusCode> {
    type Args = RelaySubsystemArgs;
    type Error = Infallible;

    fn name() -> &'static str {
        RELAY_SUBSYSTEM_NAME
    }

    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        debug!("RelaySubsystem started");

        // Create and spawn the StatePoller
        let mut poller_builder = StatePoller::builder()
            .meta(args.meta.clone())
            .backend(args.backend.clone());
        if let Some(freq) = args.backend_poll_frequency {
            poller_builder = poller_builder.freq(freq);
        }
        let mut poller = poller_builder.build();
        let state_stream = poller.take_stream().expect("State stream should be available");
        let poller_ref = poller.spawn();

        // Link to the poller
        actor_ref.link(&poller_ref).await;

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        // Spawn the main event loop as a background task
        let backend = args.backend.clone();
        let meta = args.meta.clone();
        let mut platform = args.platform;
        let mut ingress = args.ingress;
        let baseline_sender = args.baseline_sender.clone();
        let canary_sender = args.canary_sender.clone();
        let observations = args.observations;

        tokio::spawn(async move {
            let _ = Self::event_loop(
                backend,
                meta,
                &mut platform,
                &mut ingress,
                observations,
                state_stream,
                baseline_sender,
                canary_sender,
                shutdown_rx,
            )
            .await;
        });

        Ok(Self {
            backend: args.backend,
            observations: None, // Already moved to background task
            meta: args.meta,
            platform: Box::new(NoOpPlatform),
            ingress: Box::new(NoOpIngress),
            backend_poll_frequency: args.backend_poll_frequency,
            baseline_sender: args.baseline_sender,
            canary_sender: args.canary_sender,
            poller_ref: Some(poller_ref),
            shutdown_tx: Some(shutdown_tx),
        })
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        reason: ActorStopReason,
    ) -> std::result::Result<(), Self::Error> {
        debug!("RelaySubsystem stopped: {:?}", reason);
        // Send shutdown signal to background task
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn on_link_died(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        id: ActorId,
        reason: ActorStopReason,
    ) -> std::result::Result<ControlFlow<ActorStopReason>, Self::Error> {
        debug!("RelaySubsystem child actor {} died: {:?}", id, reason);
        // If our poller child dies, we should stop too
        Ok(ControlFlow::Break(ActorStopReason::LinkDied {
            id,
            reason: Box::new(reason),
        }))
    }
}

impl RelaySubsystem<StatusCode> {
    async fn event_loop(
        backend: BackendClient,
        meta: RolloutMetadata,
        platform: &mut BoxedPlatform,
        ingress: &mut BoxedIngress,
        mut observations: Receiver<Vec<StatusCode>>,
        mut state_stream: Receiver<multitool_sdk::models::RolloutState>,
        baseline_sender: Sender<String>,
        canary_sender: Sender<String>,
        mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    debug!("RelaySubsystem received shutdown signal");
                    return Ok(());
                }
                elem = observations.recv() => {
                    if let Some(batch) = elem {
                        if let Err(e) = backend.upload_observations(&meta, batch).await {
                            tracing::error!("Failed to upload observations: {}", e);
                        }
                    } else {
                        debug!("Observations stream closed");
                        return Ok(());
                    }
                }
                elem = state_stream.recv() => {
                    trace!("Received new state: {:?}", &elem);
                    if let Some(state) = elem {
                        let state_id = state.id;

                        // Create and spawn the lock manager
                        let lock_manager = match LockManager::builder()
                            .backend(backend.clone())
                            .metadata(meta.clone())
                            .state(state)
                            .build()
                            .await
                        {
                            Ok(lm) => lm,
                            Err(e) => {
                                tracing::error!("Failed to create lock manager: {}", e);
                                continue;
                            }
                        };

                        let mut locked_state = lock_manager.state().clone();
                        let _lock_ref = lock_manager.spawn();

                        // Process the state
                        let result = Self::process_state(
                            &mut locked_state,
                            platform,
                            ingress,
                            &baseline_sender,
                            &canary_sender,
                        )
                        .await;

                        match result {
                            Ok(should_shutdown) => {
                                if should_shutdown {
                                    debug!("State {} processed, shutting down", state_id);
                                    return Ok(());
                                }
                            }
                            Err(e) => {
                                tracing::error!("Failed to process state {}: {}", state_id, e);
                            }
                        }
                    } else {
                        debug!("State stream closed");
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn process_state(
        locked_state: &mut LockedState,
        platform: &mut BoxedPlatform,
        ingress: &mut BoxedIngress,
        baseline_sender: &Sender<String>,
        canary_sender: &Sender<String>,
    ) -> Result<bool> {
        let should_shutdown = match locked_state.state().state_type {
            PromoteCanary => {
                ingress.promote_canary().await?;
                locked_state.mark_done().await?;
                true // Shutdown after promotion
            }
            DeployCanary => {
                let (baseline_version_id, canary_version_id) = platform.deploy().await?;
                ingress
                    .release_canary(baseline_version_id.clone(), canary_version_id.clone())
                    .await?;
                let _ = baseline_sender.send(baseline_version_id).await;
                let _ = canary_sender.send(canary_version_id).await;
                locked_state.mark_done().await?;
                false
            }
            SetCanaryTraffic => {
                let percent_traffic = if let Some(data) = locked_state.state().data.clone().flatten()
                {
                    let RolloutStateData::RolloutStateDataOneOf(state_data) = *data;
                    state_data.set_canary_traffic.percent_traffic
                } else {
                    return Err(miette!("No data found in state"));
                };
                let percent = WholePercent::try_from(percent_traffic).unwrap();
                ingress.set_canary_traffic(percent).await?;
                locked_state.mark_done().await?;
                false
            }
            RollbackCanary => {
                ingress
                    .set_canary_traffic(WholePercent::try_from(0).unwrap())
                    .await?;
                ingress.rollback_canary().await?;
                locked_state.mark_done().await?;
                true // Shutdown after rollback
            }
            CancelCanary => {
                todo!("Cancel Canary not implemented yet in CLI");
            }
        };
        Ok(should_shutdown)
    }
}

#[bon]
#[allow(dead_code)]
impl<T: Observation + Send + 'static> RelaySubsystem<T> {
    #[builder]
    pub fn new(
        backend: BackendClient,
        meta: RolloutMetadata,
        observations: Receiver<Vec<T>>,
        platform: BoxedPlatform,
        ingress: BoxedIngress,
        backend_poll_frequency: Option<Duration>,
        baseline_sender: Sender<String>,
        canary_sender: Sender<String>,
    ) -> Self {
        debug!("Creating a new relay subsystem...");
        Self {
            backend,
            meta,
            observations: Some(observations),
            platform,
            ingress,
            backend_poll_frequency,
            baseline_sender,
            canary_sender,
            poller_ref: None,
            shutdown_tx: None,
        }
    }

    /// Spawn the relay subsystem and return the actor reference.
    /// This method converts the builder state into the Args and spawns.
    pub fn spawn_relay(self) -> ActorRef<RelaySubsystem<StatusCode>>
    where
        T: Into<StatusCode>,
    {
        let args = RelaySubsystemArgs {
            backend: self.backend,
            meta: self.meta,
            observations: unsafe {
                // SAFETY: We know T is StatusCode when this is called
                std::mem::transmute(self.observations.unwrap())
            },
            platform: self.platform,
            ingress: self.ingress,
            backend_poll_frequency: self.backend_poll_frequency,
            baseline_sender: self.baseline_sender,
            canary_sender: self.canary_sender,
        };
        RelaySubsystem::<StatusCode>::spawn_with_mailbox(args, mailbox::unbounded())
    }
}

impl RelaySubsystem<StatusCode> {
    /// Spawn the relay subsystem using the builder directly.
    pub fn spawn_with_args(args: RelaySubsystemArgs) -> ActorRef<Self> {
        Self::spawn_with_mailbox(args, mailbox::unbounded())
    }
}

// NoOp implementations for placeholder during move
use async_trait::async_trait;
use crate::adapters::backend::{IngressConfig, PlatformConfig};
use crate::adapters::{Ingress, Platform};
use crate::subsystems::{ShutdownResult, Shutdownable};

struct NoOpPlatform;

#[async_trait]
impl Platform for NoOpPlatform {
    fn get_config(&self) -> PlatformConfig {
        panic!("NoOpPlatform should not be used")
    }
    async fn deploy(&mut self) -> Result<(String, String)> {
        panic!("NoOpPlatform should not be used")
    }
    async fn yank_canary(&mut self) -> Result<()> {
        panic!("NoOpPlatform should not be used")
    }
    async fn delete_canary(&mut self) -> Result<()> {
        panic!("NoOpPlatform should not be used")
    }
    async fn promote_rollout(&mut self) -> Result<()> {
        panic!("NoOpPlatform should not be used")
    }
}

#[async_trait]
impl Shutdownable for NoOpPlatform {
    async fn shutdown(&mut self) -> ShutdownResult {
        Ok(())
    }
}

struct NoOpIngress;

#[async_trait]
impl Ingress for NoOpIngress {
    fn get_config(&self) -> IngressConfig {
        panic!("NoOpIngress should not be used")
    }
    async fn release_canary(&mut self, _: String, _: String) -> Result<()> {
        panic!("NoOpIngress should not be used")
    }
    async fn set_canary_traffic(&mut self, _: WholePercent) -> Result<()> {
        panic!("NoOpIngress should not be used")
    }
    async fn rollback_canary(&mut self) -> Result<()> {
        panic!("NoOpIngress should not be used")
    }
    async fn promote_canary(&mut self) -> Result<()> {
        panic!("NoOpIngress should not be used")
    }
}

#[async_trait]
impl Shutdownable for NoOpIngress {
    async fn shutdown(&mut self) -> ShutdownResult {
        Ok(())
    }
}

mod lock_mgmt;
mod poll_state;
