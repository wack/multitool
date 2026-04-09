use std::ops::ControlFlow;
use std::time::Duration;

use futures_util::TryStreamExt;
use kameo::actor::{ActorId, ActorRef, Spawn, WeakActorRef};
use kameo::error::{ActorStopReason, Infallible, SendError};
use kameo::mailbox;
use kameo::message::{Context, Message};
use kameo::Actor;
use miette::{Report, Result};
use tokio::{
    pin,
    sync::mpsc::{self, Receiver, Sender},
    time::interval,
};
use tokio_stream::{Stream, StreamExt as _, wrappers::IntervalStream};
use tracing::debug;

use crate::{
    adapters::{BoxedMonitor, Monitor, StatusCode},
    subsystems::TakenOptionalError,
};

use super::super::monitor::{MonitorHandle, MonitorSubsystem};

/// The maximum number of observations that can be received before we
/// emit the results to the backend.
const DEFAULT_MAX_BATCH_SIZE: usize = 512;
/// The frequency with which we poll the `Monitor` for new results.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);
/// The frequency with which we emit data from the controller.
const DEFAULT_EMIT_INTERVAL: Duration = Duration::from_secs(60);

pub const MONITOR_CONTROLLER_SUBSYSTEM_NAME: &str = "controller/monitor";

/// Arguments for MonitorController initialization.
pub struct MonitorControllerArgs {
    pub monitor: BoxedMonitor,
    pub sender: Sender<Vec<StatusCode>>,
    pub recv: Option<Receiver<Vec<StatusCode>>>,
    pub poll_interval: Duration,
    pub emit_interval: Duration,
    pub on_error: Box<dyn Fn(&miette::Report) + Send + Sync>,
    pub baseline_receiver: Receiver<String>,
    pub canary_receiver: Receiver<String>,
    pub baseline_sender: Sender<String>,
    pub canary_sender: Sender<String>,
}

/// The `MonitorController` is responsible for scheduling calls
/// to the `Monitor` on a timer, and batching the results. This
/// decouples how often we *gather* metrics from how often to
/// *store* them.
pub struct MonitorController {
    /// Handle to the child MonitorSubsystem once spawned.
    monitor_handle: Option<MonitorHandle>,
    /// Reference to the child actor for linking.
    #[allow(dead_code)]
    monitor_actor_ref: Option<ActorRef<MonitorSubsystem>>,
    /// Channel sender for batched observations.
    #[allow(dead_code)]
    sender: Sender<Vec<StatusCode>>,
    /// Channel receiver for batched observations (taken by caller).
    #[allow(dead_code)]
    recv: Option<Receiver<Vec<StatusCode>>>,
    #[allow(dead_code)]
    poll_interval: Duration,
    #[allow(dead_code)]
    emit_interval: Duration,
    #[allow(dead_code)]
    on_error: Box<dyn Fn(&miette::Report) + Send + Sync>,
    /// Receiver for baseline version ID updates.
    #[allow(dead_code)]
    baseline_receiver: Receiver<String>,
    /// Receiver for canary version ID updates.
    #[allow(dead_code)]
    canary_receiver: Receiver<String>,
    /// Sender for baseline version ID (given to caller).
    #[allow(dead_code)]
    baseline_sender: Sender<String>,
    /// Sender for canary version ID (given to caller).
    #[allow(dead_code)]
    canary_sender: Sender<String>,
}

// Manual Actor implementation since we have complex initialization
impl Actor for MonitorController {
    type Args = MonitorControllerArgs;
    type Error = Infallible;

    fn name() -> &'static str {
        MONITOR_CONTROLLER_SUBSYSTEM_NAME
    }

    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        debug!("MonitorController starting...");

        // Spawn the child MonitorSubsystem
        let monitor_actor_ref = MonitorSubsystem::spawn_actor(args.monitor);
        let monitor_handle = MonitorHandle::new(monitor_actor_ref.clone());

        // Link to the child so we're notified if it dies
        actor_ref.link(&monitor_actor_ref).await;

        // Clone monitor_handle for the spawned task
        let task_monitor_handle = monitor_handle.clone();

        // Spawn the background polling task
        let poll_interval = args.poll_interval;
        let emit_interval = args.emit_interval;
        let sender = args.sender.clone();
        let on_error: Box<dyn Fn(&Report) + Send + Sync> = Box::new(log_error);

        tokio::spawn(async move {
            let query_stream = repeat_query(Box::new(task_monitor_handle), poll_interval)
                .inspect_err(|e| (on_error)(e))
                .filter_map(Result::ok);

            let chunked_stream =
                query_stream.chunks_timeout(DEFAULT_MAX_BATCH_SIZE, emit_interval);

            pin!(chunked_stream);

            while let Some(batch) = chunked_stream.next().await {
                if sender.send(batch).await.is_err() {
                    // Receiver dropped, stop polling
                    break;
                }
            }
        });

        Ok(Self {
            monitor_handle: Some(monitor_handle),
            monitor_actor_ref: Some(monitor_actor_ref),
            sender: args.sender,
            recv: args.recv,
            poll_interval: args.poll_interval,
            emit_interval: args.emit_interval,
            on_error: args.on_error,
            baseline_receiver: args.baseline_receiver,
            canary_receiver: args.canary_receiver,
            baseline_sender: args.baseline_sender,
            canary_sender: args.canary_sender,
        })
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        reason: ActorStopReason,
    ) -> std::result::Result<(), Self::Error> {
        debug!("MonitorController stopped: {:?}", reason);
        Ok(())
    }

    async fn on_link_died(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        id: ActorId,
        reason: ActorStopReason,
    ) -> std::result::Result<ControlFlow<ActorStopReason>, Self::Error> {
        debug!("MonitorController child actor {} died: {:?}", id, reason);
        // If our monitor child dies, we should stop too
        Ok(ControlFlow::Break(ActorStopReason::LinkDied {
            id,
            reason: Box::new(reason),
        }))
    }
}

/// Builder for MonitorController that collects configuration before spawning.
pub struct MonitorControllerBuilder {
    monitor: Option<BoxedMonitor>,
    poll_interval: Option<Duration>,
    emit_interval: Option<Duration>,
    sender: Option<Sender<Vec<StatusCode>>>,
    recv: Option<Receiver<Vec<StatusCode>>>,
    baseline_sender: Option<Sender<String>>,
    canary_sender: Option<Sender<String>>,
    baseline_receiver: Option<Receiver<String>>,
    canary_receiver: Option<Receiver<String>>,
}

impl MonitorController {
    pub fn builder() -> MonitorControllerBuilderStep1 {
        MonitorControllerBuilderStep1 { }
    }
}

/// Step 1 of the builder - needs monitor.
pub struct MonitorControllerBuilderStep1 {}

impl MonitorControllerBuilderStep1 {
    pub fn monitor(self, monitor: BoxedMonitor) -> MonitorControllerBuilder {
        let (sender, receiver) = mpsc::channel(DEFAULT_MAX_BATCH_SIZE);
        let (baseline_sender, baseline_receiver) = mpsc::channel(DEFAULT_MAX_BATCH_SIZE);
        let (canary_sender, canary_receiver) = mpsc::channel(DEFAULT_MAX_BATCH_SIZE);

        MonitorControllerBuilder {
            monitor: Some(monitor),
            poll_interval: None,
            emit_interval: None,
            sender: Some(sender),
            recv: Some(receiver),
            baseline_sender: Some(baseline_sender),
            canary_sender: Some(canary_sender),
            baseline_receiver: Some(baseline_receiver),
            canary_receiver: Some(canary_receiver),
        }
    }
}

impl MonitorControllerBuilder {
    /// Finish building and return self (allows chaining before spawn).
    pub fn build(self) -> Self {
        self
    }

    /// Returns a channel receiver of batched observations.
    /// Can only be called once before spawning.
    pub fn stream(&mut self) -> Result<Receiver<Vec<StatusCode>>> {
        self.recv.take().ok_or(TakenOptionalError.into())
    }

    pub fn get_baseline_sender(&self) -> Sender<String> {
        self.baseline_sender.clone().expect("baseline_sender already taken")
    }

    pub fn get_canary_sender(&self) -> Sender<String> {
        self.canary_sender.clone().expect("canary_sender already taken")
    }

    /// Spawn the controller and return the actor reference.
    pub fn spawn(mut self) -> ActorRef<MonitorController> {
        let args = MonitorControllerArgs {
            monitor: self.monitor.take().expect("monitor is required"),
            sender: self.sender.take().expect("sender already taken"),
            recv: self.recv.take(),
            poll_interval: self.poll_interval.unwrap_or(DEFAULT_POLL_INTERVAL),
            emit_interval: self.emit_interval.unwrap_or(DEFAULT_EMIT_INTERVAL),
            on_error: Box::new(log_error),
            baseline_receiver: self.baseline_receiver.take().expect("baseline_receiver already taken"),
            canary_receiver: self.canary_receiver.take().expect("canary_receiver already taken"),
            baseline_sender: self.baseline_sender.take().expect("baseline_sender already taken"),
            canary_sender: self.canary_sender.take().expect("canary_sender already taken"),
        };
        MonitorController::spawn_with_mailbox(args, mailbox::unbounded())
    }
}

// --- Messages ---

/// Message to set the baseline version ID.
pub struct SetBaselineVersionId {
    pub version_id: String,
}

impl Message<SetBaselineVersionId> for MonitorController {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: SetBaselineVersionId,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Some(ref mut handle) = self.monitor_handle {
            handle.set_baseline_version_id(msg.version_id).await
        } else {
            Err(miette::miette!("Monitor not initialized"))
        }
    }
}

/// Message to set the canary version ID.
pub struct SetCanaryVersionId {
    pub version_id: String,
}

impl Message<SetCanaryVersionId> for MonitorController {
    type Reply = Result<()>;

    async fn handle(
        &mut self,
        msg: SetCanaryVersionId,
        _ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Some(ref mut handle) = self.monitor_handle {
            handle.set_canary_version_id(msg.version_id).await
        } else {
            Err(miette::miette!("Monitor not initialized"))
        }
    }
}

/// Handle to the MonitorController that provides the same interface
/// as the old channel-based approach.
#[derive(Clone)]
#[allow(dead_code)]
pub struct MonitorControllerHandle {
    actor_ref: ActorRef<MonitorController>,
}

#[allow(dead_code)]
impl MonitorControllerHandle {
    pub fn new(actor_ref: ActorRef<MonitorController>) -> Self {
        Self { actor_ref }
    }

    pub fn actor_ref(&self) -> &ActorRef<MonitorController> {
        &self.actor_ref
    }

    pub async fn set_baseline_version_id(&self, version_id: String) -> Result<()> {
        self.actor_ref
            .ask(SetBaselineVersionId { version_id })
            .await
            .map_err(|e: SendError<_, _>| {
                miette::miette!("Failed to send to monitor controller: {:?}", e)
            })?;
        Ok(())
    }

    pub async fn set_canary_version_id(&self, version_id: String) -> Result<()> {
        self.actor_ref
            .ask(SetCanaryVersionId { version_id })
            .await
            .map_err(|e: SendError<_, _>| {
                miette::miette!("Failed to send to monitor controller: {:?}", e)
            })?;
        Ok(())
    }
}

fn log_error(err: &Report) {
    tracing::error!("Error while collecting monitoring data: {err}");
}

/// [repeat_query] runs the query on an interval and returns a stream of items.
fn repeat_query(
    mut monitor: BoxedMonitor,
    duration: tokio::time::Duration,
) -> impl Stream<Item = Result<StatusCode>> {
    async_stream::stream! {
        let timer = IntervalStream::new(interval(duration));
        pin!(timer);
        while timer.next().await.is_some() {
            match monitor.query().await {
                Ok(items) => {
                    for item in items {
                        yield Ok(item);
                    }
                },
                Err(err) => yield Err(err),
            }
        }
    }
}
