//! The live-presentation phase (MULTI-1369): a fourth Kameo actor that surfaces
//! what `multi check` is doing **while it runs**.
//!
//! Unlike the three pipeline actors, [`PresenterActor`] does **not** sit in the
//! `tell` chain between phases. It is display-only: the pipeline actors each hold
//! an `ActorRef<PresenterActor>` and fire-and-forget a [`UiEvent`] at each
//! milestone (discovered → running → retrying → settled). The presenter folds
//! those into a [`PresenterState`] view-model and renders through a
//! [`RenderBackend`] chosen at spawn from whether stdout is a TTY:
//!
//! * TTY → [`InlineTuiBackend`]: a live requirement→check tree + gauge in an
//!   inline viewport, flushing completed requirements into scrollback.
//! * not a TTY → [`HeartbeatBackend`]: a periodic stderr progress line; stdout is
//!   left untouched for the reporting actor's byte-for-byte report.
//! * tests → [`NullBackend`] / `RecordingBackend`: no terminal.
//!
//! Events are a concrete enum (exhaustiveness, a single `Message` impl, no
//! per-message boxing); the polymorphism lives on the backend. A separate
//! periodic [`Tick`] drives `backend.tick()` so spinners and elapsed timers keep
//! moving between events — the liveness signal that proves a slow run isn't hung.
//!
//! The presenter also owns `tracing` output for the run's duration: `on_start`
//! registers itself via [`crate::terminal::route_logs`] and pumps every routed
//! line in as a [`UiEvent::Log`], so a `tracing::info!` fired mid-run reaches the
//! same backend instead of writing raw bytes straight to stdout — which, for
//! [`InlineTuiBackend`], would corrupt its cursor-managed viewport. `on_stop`
//! drops the registration, restoring direct stdout logging.
//!
//! [`InlineTuiBackend`]: inline::InlineTuiBackend
//! [`HeartbeatBackend`]: heartbeat::HeartbeatBackend
//! [`NullBackend`]: recording::NullBackend

mod backend;
mod format;
mod heartbeat;
mod inline;
mod recording;
mod state;

use std::convert::Infallible;
use std::io::IsTerminal;

use kameo::Actor;
use kameo::actor::{ActorRef, WeakActorRef};
use kameo::error::ActorStopReason;
use kameo::message::{Context, Message};

use crate::checks::model::{CheckId, CheckOutcome};
use crate::terminal::{LogRouteGuard, route_logs};

pub(crate) use backend::RenderBackend;
use heartbeat::HeartbeatBackend;
use inline::InlineTuiBackend;
use recording::NullBackend;
use state::PresenterState;

#[cfg(test)]
use recording::RecordingBackend;

/// A milestone emitted by a pipeline actor to the presenter. Fire-and-forget via
/// `tell`. A concrete enum so every variant is rendered explicitly (a new variant
/// forces every backend's match to update) with no dynamic dispatch per message.
#[derive(Clone, Debug)]
pub(crate) enum UiEvent {
    /// Discovery finished streaming; `total_checks` is the gauge denominator.
    DiscoveryComplete { total_checks: usize },
    /// A validated check was enqueued. Carries grouping/labels so the presenter
    /// can build the tree without consulting the suite.
    CheckQueued {
        id: CheckId,
        req_index: usize,
        req_title: String,
        check_title: String,
    },
    /// A permit was acquired and the check's agent began running.
    CheckStarted { id: CheckId },
    /// A prior attempt finished without a verdict; the check is being re-run. The
    /// `attempt` is the just-finished attempt number.
    CheckRetrying { id: CheckId, attempt: u32 },
    /// The check reached a terminal verdict. Carries the reconciled outcome so the
    /// presenter can render the same record the reporting actor would.
    CheckSettled { id: CheckId, outcome: CheckOutcome },
    /// A `tracing` log line fired somewhere in the run, already formatted.
    /// Routed through the presenter (via [`crate::terminal::route_logs`]) so it
    /// never writes raw bytes over a live backend's cursor-managed display; each
    /// backend decides for itself where a log line belongs.
    Log(String),
}

/// The periodic redraw nudge. Decoupled from [`UiEvent`] so rendering is
/// rate-limited and animations keep moving even when no events arrive.
pub(crate) struct Tick;

/// The display-only presenter actor. Owns the view-model and the active backend.
pub(crate) struct PresenterActor {
    state: PresenterState,
    backend: Box<dyn RenderBackend>,
    /// The redraw ticker task, joined on stop so it never outlives the actor.
    ticker: Option<tokio::task::JoinHandle<()>>,
    /// The task pumping routed log lines into `self` as [`UiEvent::Log`]s.
    /// Joined on stop, after `log_route` is dropped closes its channel.
    log_pump: Option<tokio::task::JoinHandle<()>>,
    /// Keeps this actor registered as `tracing`'s active sink; dropping it (in
    /// `on_stop`) restores direct stdout logging and closes `log_pump`'s channel.
    log_route: Option<LogRouteGuard>,
}

impl PresenterActor {
    /// Build the actor over a chosen backend, for a run against `model`.
    pub(crate) fn new(backend: Box<dyn RenderBackend>, model: String) -> Self {
        Self {
            state: PresenterState::new(model),
            backend,
            ticker: None,
            log_pump: None,
            log_route: None,
        }
    }
}

impl Actor for PresenterActor {
    type Args = Self;
    type Error = Infallible;

    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        // Drive redraws from a background ticker. It holds only a *weak* ref
        // between iterations (so it can't keep the actor alive), and races each
        // tick against the actor's shutdown so it exits the instant the run ends
        // rather than parking until the next (possibly distant) interval.
        let interval = args.backend.tick_interval();
        let weak = actor_ref.downgrade();
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                let Some(actor) = weak.upgrade() else {
                    break;
                };
                tokio::select! {
                    _ = ticker.tick() => {
                        if actor.tell(Tick).await.is_err() {
                            break;
                        }
                    }
                    _ = actor.wait_for_shutdown() => break,
                }
            }
        });

        // Become tracing's active sink for the run's duration and pump routed
        // lines in as `UiEvent::Log`s, mirroring the ticker's weak-ref pattern so
        // this task can't keep the actor alive and exits the instant it's gone.
        let (log_tx, mut log_rx) = tokio::sync::mpsc::unbounded_channel();
        let log_route = route_logs(log_tx);
        let weak_log = actor_ref.downgrade();
        let log_handle = tokio::spawn(async move {
            while let Some(line) = log_rx.recv().await {
                let Some(actor) = weak_log.upgrade() else {
                    break;
                };
                if actor.tell(UiEvent::Log(line)).await.is_err() {
                    break;
                }
            }
        });

        let mut me = args;
        me.ticker = Some(handle);
        me.log_pump = Some(log_handle);
        me.log_route = Some(log_route);
        Ok(me)
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        _reason: ActorStopReason,
    ) -> std::result::Result<(), Self::Error> {
        // Stop the ticker and wait for it to unwind before returning, so it never
        // outlives the actor (which would read as a leaked task in tests).
        if let Some(handle) = self.ticker.take() {
            handle.abort();
            let _ = handle.await;
        }
        // Unregister first so tracing falls back to stdout again, which closes
        // the pump's channel; then wait for it to drain whatever was already
        // queued and exit on its own rather than aborting it mid-line.
        self.log_route.take();
        if let Some(handle) = self.log_pump.take() {
            let _ = handle.await;
        }
        // The authoritative restore: runs on graceful stop, kill, *and* panic
        // (in addition to the panic hook that shows the cursor).
        self.backend.teardown(&self.state);
        Ok(())
    }
}

impl Message<UiEvent> for PresenterActor {
    type Reply = ();

    async fn handle(&mut self, event: UiEvent, _ctx: &mut Context<Self, ()>) -> Self::Reply {
        // Mutate the view-model first, then let the backend react to the event.
        self.state.apply(&event);
        self.backend.apply(&self.state, &event);
    }
}

impl Message<Tick> for PresenterActor {
    type Reply = ();

    async fn handle(&mut self, _tick: Tick, _ctx: &mut Context<Self, ()>) -> Self::Reply {
        self.backend.tick(&self.state);
    }
}

/// The chosen backend plus whether it **owns the record** — i.e. flushes the
/// per-requirement results to the terminal itself. Only the inline TUI does; when
/// it can't be set up the reporting actor must still write the record to stdout.
pub(crate) struct Backend {
    pub backend: Box<dyn RenderBackend>,
    /// True only for the inline TUI: the presenter is the sole terminal writer, so
    /// [`crate::checks::run`] must *not* also print the report (it would double).
    pub owns_record: bool,
}

/// Choose the backend for a real run from the terminal environment: the inline
/// TUI when stdout is a TTY (falling back to the heartbeat if terminal setup
/// fails), otherwise the stderr heartbeat.
pub(crate) fn select_backend(color: bool) -> Backend {
    let stderr_is_tty = std::io::stderr().is_terminal();
    if std::io::stdout().is_terminal() {
        // Only the successfully-initialised inline TUI owns the record; a failed
        // setup degrades to the heartbeat, and then stdout still needs the report.
        if let Ok(backend) = InlineTuiBackend::new(color) {
            return Backend {
                backend: Box::new(backend),
                owns_record: true,
            };
        }
    }
    Backend {
        backend: Box::new(HeartbeatBackend::new(stderr_is_tty)),
        owns_record: false,
    }
}

/// A no-op backend: spawned by the pipeline tests (and the future `--quiet`).
pub(crate) fn null_backend() -> Box<dyn RenderBackend> {
    Box::new(NullBackend)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::model::Verdict;
    use kameo::actor::Spawn;
    use std::sync::{Arc, Mutex};

    /// End-to-end through the actor: events drive state and reach the backend, and
    /// graceful stop runs teardown — exercised via the recording backend.
    #[tokio::test]
    async fn actor_folds_events_and_records_them() {
        let (backend, events) = RecordingBackend::new();
        let presenter =
            PresenterActor::spawn(PresenterActor::new(Box::new(backend), "test-model".into()));

        presenter
            .tell(UiEvent::CheckQueued {
                id: 0,
                req_index: 0,
                req_title: "R".into(),
                check_title: "c".into(),
            })
            .await
            .unwrap();
        presenter
            .tell(UiEvent::DiscoveryComplete { total_checks: 1 })
            .await
            .unwrap();
        presenter
            .tell(UiEvent::CheckStarted { id: 0 })
            .await
            .unwrap();
        presenter
            .tell(UiEvent::CheckSettled {
                id: 0,
                outcome: CheckOutcome {
                    title: "c".into(),
                    verdict: Verdict::Satisfied,
                    evidence: None,
                },
            })
            .await
            .unwrap();

        presenter.stop_gracefully().await.unwrap();
        presenter.wait_for_shutdown().await;

        let recorded: Arc<Mutex<Vec<UiEvent>>> = events;
        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.len(), 4);
        assert!(matches!(recorded[0], UiEvent::CheckQueued { id: 0, .. }));
        assert!(matches!(
            recorded[3],
            UiEvent::CheckSettled {
                id: 0,
                outcome: CheckOutcome {
                    verdict: Verdict::Satisfied,
                    ..
                }
            }
        ));
    }
}
