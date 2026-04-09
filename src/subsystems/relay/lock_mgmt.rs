use kameo::actor::{ActorRef, Spawn, WeakActorRef};
use kameo::error::{ActorStopReason, Infallible};
use kameo::mailbox;
use kameo::message::{Context, Message};
use kameo::Actor;
use miette::Result;
use multitool_sdk::models::RolloutState;
use tokio::sync::mpsc::{self, Receiver};
use tokio::sync::oneshot;
use tokio::time::{interval, Duration};
use tracing::debug;

use crate::{
    adapters::{BackendClient, RolloutMetadata},
    subsystems::ShutdownResult,
};

use super::LockedState;

/// Arguments for LockManager initialization.
pub(super) struct LockManagerArgs {
    pub backend: BackendClient,
    pub meta: RolloutMetadata,
    pub state: LockedState,
    pub freq: Duration,
    pub task_done: Receiver<oneshot::Sender<()>>,
}

/// The LockManager is responsible for maintaining a lock on a state
/// while it's being processed, and marking it done when complete.
pub(super) struct LockManager {
    /// We use this client to refresh locks.
    backend: BackendClient,
    /// This field describes the current active rollout.
    meta: RolloutMetadata,
    /// This is the state that this manager is locking.
    state: LockedState,
    /// This channel is filled when the state has been effected
    /// by the ingress/platform.
    #[allow(dead_code)]
    task_done: Receiver<oneshot::Sender<()>>,
}

impl Actor for LockManager {
    type Args = LockManagerArgs;
    type Error = Infallible;

    fn name() -> &'static str {
        "LockManager"
    }

    async fn on_start(
        args: Self::Args,
        _actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        debug!("LockManager started for state {}", args.state.state().id);

        // Spawn the background lock refresh task
        let backend = args.backend.clone();
        let meta = args.meta.clone();
        let state = args.state.clone();
        let freq = args.freq;

        tokio::spawn(async move {
            let mut timer = interval(freq);
            loop {
                timer.tick().await;
                if let Err(e) = backend.refresh_lock(&meta, &state).await {
                    tracing::error!("Failed to refresh lock: {}", e);
                    // Don't break - keep trying
                }
            }
        });

        Ok(Self {
            backend: args.backend,
            meta: args.meta,
            state: args.state,
            task_done: args.task_done,
        })
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        reason: ActorStopReason,
    ) -> std::result::Result<(), Self::Error> {
        debug!("LockManager stopping for state {}", self.state.state().id);

        // If stopping due to abnormal reasons, abandon the lock
        match reason {
            ActorStopReason::Normal => {
                // Normal stop means we completed successfully
            }
            _ => {
                // Abnormal stop - release the lock
                if let Err(e) = self.backend.abandon_lock(&self.meta, &self.state).await {
                    tracing::error!("Failed to abandon lock on shutdown: {}", e);
                }
            }
        }

        Ok(())
    }
}

impl LockManager {
    pub(super) fn builder() -> LockManagerBuilderStep1 {
        LockManagerBuilderStep1 {}
    }

    #[allow(dead_code)]
    pub(super) fn state(&self) -> &LockedState {
        &self.state
    }
}

pub(super) struct LockManagerBuilderStep1 {}

impl LockManagerBuilderStep1 {
    pub fn backend(self, backend: BackendClient) -> LockManagerBuilderStep2 {
        LockManagerBuilderStep2 { backend }
    }
}

pub(super) struct LockManagerBuilderStep2 {
    backend: BackendClient,
}

impl LockManagerBuilderStep2 {
    pub fn metadata(self, metadata: RolloutMetadata) -> LockManagerBuilderStep3 {
        LockManagerBuilderStep3 {
            backend: self.backend,
            metadata,
        }
    }
}

pub(super) struct LockManagerBuilderStep3 {
    backend: BackendClient,
    metadata: RolloutMetadata,
}

impl LockManagerBuilderStep3 {
    pub fn state(self, state: RolloutState) -> LockManagerBuilderStep4 {
        LockManagerBuilderStep4 {
            backend: self.backend,
            metadata: self.metadata,
            state,
        }
    }
}

pub(super) struct LockManagerBuilderStep4 {
    backend: BackendClient,
    metadata: RolloutMetadata,
    state: RolloutState,
}

impl LockManagerBuilderStep4 {
    pub async fn build(self) -> Result<LockManagerBuilder> {
        let (done_sender, task_done) = mpsc::channel(1);
        // Take the initial lock.
        let locked_state = self.backend.lock_state(&self.metadata, &self.state, done_sender).await?;
        let freq = *locked_state.frequency();
        Ok(LockManagerBuilder {
            backend: self.backend,
            meta: self.metadata,
            state: locked_state,
            freq: freq / 2,
            task_done,
        })
    }
}

/// Builder for LockManager that holds the pre-locked state.
pub(super) struct LockManagerBuilder {
    backend: BackendClient,
    meta: RolloutMetadata,
    state: LockedState,
    freq: Duration,
    task_done: Receiver<oneshot::Sender<()>>,
}

impl LockManagerBuilder {
    pub(super) fn state(&self) -> &LockedState {
        &self.state
    }

    /// Spawn the lock manager and return the actor reference.
    pub fn spawn(self) -> ActorRef<LockManager> {
        let args = LockManagerArgs {
            backend: self.backend,
            meta: self.meta,
            state: self.state,
            freq: self.freq,
            task_done: self.task_done,
        };
        LockManager::spawn_with_mailbox(args, mailbox::unbounded())
    }
}

// --- Messages ---

/// Message to mark the task as done and complete the lock.
pub struct MarkDone;

impl Message<MarkDone> for LockManager {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        _msg: MarkDone,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        // Mark the state as completed with the backend
        self.backend
            .mark_state_completed(&self.meta, &self.state)
            .await?;

        // Stop the actor after completing
        ctx.stop();

        Ok(())
    }
}

/// Message to abandon the lock (for abnormal shutdown).
pub struct AbandonLock;

impl Message<AbandonLock> for LockManager {
    type Reply = ShutdownResult;

    async fn handle(
        &mut self,
        _msg: AbandonLock,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.backend.abandon_lock(&self.meta, &self.state).await?;
        ctx.stop();
        Ok(())
    }
}
