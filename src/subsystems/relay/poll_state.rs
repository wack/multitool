use kameo::actor::{ActorRef, Spawn, WeakActorRef};
use kameo::error::{ActorStopReason, Infallible};
use kameo::mailbox;
use kameo::Actor;
use miette::Result;
use multitool_sdk::models::RolloutState;
use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, interval},
};
use tracing::debug;

use crate::{
    adapters::{BackendClient, RolloutMetadata},
    subsystems::TakenOptionalError,
};

/// This is the amount of time between calls to the backend to
/// refresh the list of states that need to be applied.
const DEFAULT_POLLING_FREQUENCY: Duration = Duration::from_secs(10);
/// If you're going to pick an arbitrary number, you could do worse
/// than picking a power of two.
const DEFAULT_CHANNEL_SIZE: usize = 1 << 5;

/// Arguments for StatePoller initialization.
pub struct StatePollerArgs {
    pub meta: RolloutMetadata,
    pub backend: BackendClient,
    pub freq: Duration,
    pub outbox: Sender<RolloutState>,
    pub stream: Option<Receiver<RolloutState>>,
}

/// The StatePoller periodically polls the backend for new rollout states.
pub struct StatePoller {
    /// This is the client we use to poll for new state.
    #[allow(dead_code)]
    backend: BackendClient,
    /// This field describes the current active rollout. It's
    /// context we pass to the backend on each request.
    #[allow(dead_code)]
    meta: RolloutMetadata,
    /// This is where we write new messages when we have them.
    #[allow(dead_code)]
    outbox: Sender<RolloutState>,
    /// We give this to the caller so it can stream new messages.
    #[allow(dead_code)]
    stream: Option<Receiver<RolloutState>>,
}

impl Actor for StatePoller {
    type Args = StatePollerArgs;
    type Error = Infallible;

    fn name() -> &'static str {
        "StatePoller"
    }

    async fn on_start(
        args: Self::Args,
        _actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        debug!("StatePoller started");

        // Spawn the background polling task
        let backend = args.backend.clone();
        let meta = args.meta.clone();
        let outbox = args.outbox.clone();
        let freq = args.freq;

        tokio::spawn(async move {
            let mut timer = interval(freq);
            loop {
                timer.tick().await;

                match backend.poll_for_state(&meta).await {
                    Ok(states) => {
                        for state in states {
                            if outbox.send(state).await.is_err() {
                                // Receiver dropped, stop polling
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error polling for state: {}", e);
                    }
                }
            }
        });

        Ok(Self {
            backend: args.backend,
            meta: args.meta,
            outbox: args.outbox,
            stream: args.stream,
        })
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        reason: ActorStopReason,
    ) -> std::result::Result<(), Self::Error> {
        debug!("StatePoller stopped: {:?}", reason);
        Ok(())
    }
}

/// Builder for StatePoller that collects configuration before spawning.
pub struct StatePollerBuilder {
    meta: Option<RolloutMetadata>,
    backend: Option<BackendClient>,
    freq: Option<Duration>,
    outbox: Option<Sender<RolloutState>>,
    stream: Option<Receiver<RolloutState>>,
}

/// Step 1 of StatePoller builder - needs meta and backend.
pub struct StatePollerBuilderStep1 {}

impl StatePoller {
    pub fn builder() -> StatePollerBuilderStep1 {
        StatePollerBuilderStep1 {}
    }

    #[allow(dead_code)]
    pub fn take_stream(&mut self) -> Result<Receiver<RolloutState>> {
        self.stream.take().ok_or(TakenOptionalError.into())
    }
}

impl StatePollerBuilderStep1 {
    pub fn meta(self, meta: RolloutMetadata) -> StatePollerBuilderStep2 {
        StatePollerBuilderStep2 { meta }
    }
}

pub struct StatePollerBuilderStep2 {
    meta: RolloutMetadata,
}

impl StatePollerBuilderStep2 {
    pub fn backend(self, backend: BackendClient) -> StatePollerBuilder {
        let (outbox, inbox) = mpsc::channel(DEFAULT_CHANNEL_SIZE);
        StatePollerBuilder {
            meta: Some(self.meta),
            backend: Some(backend),
            freq: None,
            outbox: Some(outbox),
            stream: Some(inbox),
        }
    }
}

impl StatePollerBuilder {
    pub fn freq(mut self, freq: Duration) -> Self {
        self.freq = Some(freq);
        self
    }

    pub fn build(self) -> Self {
        self
    }

    pub fn take_stream(&mut self) -> Result<Receiver<RolloutState>> {
        self.stream.take().ok_or(TakenOptionalError.into())
    }

    /// Spawn the state poller and return the actor reference.
    pub fn spawn(mut self) -> ActorRef<StatePoller> {
        let args = StatePollerArgs {
            meta: self.meta.take().expect("meta is required"),
            backend: self.backend.take().expect("backend is required"),
            freq: self.freq.unwrap_or(DEFAULT_POLLING_FREQUENCY),
            outbox: self.outbox.take().expect("outbox already taken"),
            stream: self.stream.take(),
        };
        StatePoller::spawn_with_mailbox(args, mailbox::unbounded())
    }
}
