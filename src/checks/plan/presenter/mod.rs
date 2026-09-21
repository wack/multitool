//! The live-presentation phase for `multi plan` (MULTI-1829): display-only,
//! event-driven, and — per the ticket — driving its **own** event stream from
//! `checks::plan` rather than forcing plan-specific states into `multi
//! check`'s [`UiEvent`]/[`CheckState`] (`crate::checks::presenter`).
//!
//! The split mirrors `crate::checks::presenter` closely (same actor shape,
//! same TTY/non-TTY backend selection, same `Tick`-driven redraw) but keeps
//! its own [`PlanUiEvent`] enum, [`state::PlanRowState`]/
//! [`state::PlanPresenterState`] view-model, and backends — so a new plan row
//! state can never force `multi check`'s exhaustive `match`es to grow a case
//! they have no use for, and vice versa. What **is** shared, deliberately
//! narrow: the pure formatting helpers in `crate::checks::presenter::format`
//! (spinner, elapsed-time, gauge) and two small terminal-mechanics primitives
//! from `crate::checks::presenter::inline`
//! ([`crate::checks::presenter::install_terminal_guards`],
//! [`crate::checks::presenter::styled_wrapped_lines`]) — never the backend
//! structs or the check-side `RenderBackend` trait themselves, which stay
//! untouched (`multi check` rendering is unaffected by this ticket).
//!
//! Like `crate::checks::presenter`, this is display-only: [`plan::mod`] holds
//! an [`Option<PlanEventSink>`] and fire-and-forgets a [`PlanUiEvent`] at each
//! milestone; if the actor is never spawned (every existing orchestration
//! test — see `plan::run_with_planner`'s `presentation: None`) or dies
//! mid-run, planning itself is entirely unaffected — only the live view is
//! missing.
//!
//! [`plan::mod`]: super

mod heartbeat;
mod inline;
mod state;

use std::io::IsTerminal;

use kameo::Actor;
use kameo::actor::{ActorRef, WeakActorRef};
use kameo::error::ActorStopReason;
use kameo::message::{Context, Message};

use crate::checks::jev::plan_file::AgentReason;
use crate::checks::model::CheckId;
use crate::terminal::{LogRouteGuard, route_logs};

use heartbeat::PlanHeartbeatBackend;
use inline::PlanInlineTuiBackend;
pub(crate) use state::{PlanPresenterState, StaleReason};

/// A milestone emitted by `checks::plan`'s orchestration
/// (`plan::run_with_planner`/`plan::process_check`) or planner
/// (`plan::planner::AgentPlanner`) to the plan presenter. Fire-and-forget via
/// `tell`, mirroring `crate::checks::presenter::UiEvent`'s shape and reasons
/// for being a concrete, exhaustively-matched enum.
#[derive(Clone, Debug)]
pub(crate) enum PlanUiEvent {
    /// Every check to plan this run is now known; `total_checks` is the
    /// gauge denominator (refused-file checks count too — see
    /// `plan::RunReport::total_checks`).
    DiscoveryComplete { total_checks: usize },
    /// A discovered check was enqueued — emitted for **every** check up
    /// front (the ticket: "shown immediately so the full list of
    /// requirements is visible up front"), before any cache lookup or
    /// planning begins.
    Queued {
        id: CheckId,
        req_index: usize,
        req_title: String,
        check_title: String,
    },
    /// Replaying the existing plan's tool calls and comparing checksums (or,
    /// for a check with no matching cached entry, about to be classified
    /// `Stale`).
    Verifying { id: CheckId },
    /// Checksums matched: the plan is reused, no agent runs. `truncated` is
    /// whether the reused entry's frozen calls carry a
    /// [`crate::checks::jev::plan_file::PlanCall::Truncated`] (the footer's
    /// "N truncated" counts these too, exactly as
    /// `plan::entry_has_truncated_call` does for the summary line).
    Fresh { id: CheckId, truncated: bool },
    /// The plan is stale (or missing, or `--force`d) and about to be
    /// (re-)planned. `reason` is retained on the row (not repeated on later
    /// events) so `Planning`/`Retrying`/`Calibrating` can keep showing it.
    Stale { id: CheckId, reason: StaleReason },
    /// The agent is now running this attempt (1-based). Mirrors
    /// `UiEvent::CheckStarted`: `attempt` lets the row distinguish this
    /// attempt's `Progress` from a superseded one's.
    Planning { id: CheckId, attempt: u32 },
    /// A prior attempt finished without a verdict; about to retry. `attempt`
    /// is the just-finished attempt number — mirrors `UiEvent::CheckRetrying`.
    Retrying { id: CheckId, attempt: u32 },
    /// In-flight agent progress (MULTI-1828's `AgentProgress`, forwarded the
    /// same fire-and-forget, never-awaited way `checks::execution::run_one`
    /// forwards it for `multi check` — see `plan::planner::AgentPlanner`).
    Progress {
        id: CheckId,
        attempt: u32,
        turn: u32,
        max_turns: u32,
        activity: Option<String>,
    },
    /// Asking Jev to reproduce the agent's verdict (and, unless that alone is
    /// already over budget, to reject the empty-evidence negative control).
    Calibrating { id: CheckId },
    /// Terminal: `decider = "jev"` — Jev reproduced the agent's verdict and
    /// rejected the control. `verdict` is the agent's pass/fail.
    Planned {
        id: CheckId,
        verdict: bool,
        truncated: bool,
    },
    /// Terminal: `decider = "agent"` — settled by the reasoning agent's own
    /// verdict, with `reason` naming why Jev wasn't trusted (or wasn't asked).
    AgentOnly {
        id: CheckId,
        reason: AgentReason,
        verdict: bool,
        truncated: bool,
    },
    /// Terminal: the agent never reported a verdict (or errored), or the
    /// check's file has no manifest-derived root (refused — see
    /// `plan::partition_checks`'s docs).
    Error { id: CheckId, message: String },
    /// Every `.check-plan.toml` written this run, for the final record.
    PlanFilesWritten(Vec<std::path::PathBuf>),
    /// A routed `tracing` log line — see `crate::checks::presenter::UiEvent::Log`'s
    /// docs; the plan presenter becomes `tracing`'s sink identically.
    Log(String),
}

/// The periodic redraw nudge — identical role to
/// `crate::checks::presenter::Tick`, kept as its own type so the two
/// presenters' mailboxes are never confused for one another.
pub(crate) struct Tick;

/// The display-only plan presenter actor. Owns the view-model and the active
/// backend — structurally identical to
/// `crate::checks::presenter::PresenterActor` (ticker + routed-log pump), just
/// over [`PlanPresenterState`]/[`PlanUiEvent`] instead.
pub(crate) struct PlanPresenterActor {
    state: PlanPresenterState,
    backend: Box<dyn PlanRenderBackend>,
    ticker: Option<tokio::task::JoinHandle<()>>,
    log_pump: Option<tokio::task::JoinHandle<()>>,
    log_route: Option<LogRouteGuard>,
}

impl PlanPresenterActor {
    pub(crate) fn new(backend: Box<dyn PlanRenderBackend>, model: String) -> Self {
        Self {
            state: PlanPresenterState::new(model),
            backend,
            ticker: None,
            log_pump: None,
            log_route: None,
        }
    }
}

impl Actor for PlanPresenterActor {
    type Args = Self;
    type Error = std::convert::Infallible;

    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
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

        let (log_tx, mut log_rx) = tokio::sync::mpsc::unbounded_channel();
        let log_route = route_logs(log_tx);
        let weak_log = actor_ref.downgrade();
        let log_handle = tokio::spawn(async move {
            while let Some(line) = log_rx.recv().await {
                let Some(actor) = weak_log.upgrade() else {
                    break;
                };
                if actor.tell(PlanUiEvent::Log(line)).await.is_err() {
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
        if let Some(handle) = self.ticker.take() {
            handle.abort();
            let _ = handle.await;
        }
        self.log_route.take();
        if let Some(handle) = self.log_pump.take() {
            let _ = handle.await;
        }
        self.backend.teardown(&self.state);
        Ok(())
    }
}

impl Message<PlanUiEvent> for PlanPresenterActor {
    type Reply = ();

    async fn handle(&mut self, event: PlanUiEvent, _ctx: &mut Context<Self, ()>) -> Self::Reply {
        self.state.apply(&event);
        self.backend.apply(&self.state, &event);
    }
}

impl Message<Tick> for PlanPresenterActor {
    type Reply = ();

    async fn handle(&mut self, _tick: Tick, _ctx: &mut Context<Self, ()>) -> Self::Reply {
        self.backend.tick(&self.state);
    }
}

/// A render target for the live plan presenter — the plan-side counterpart of
/// `crate::checks::presenter::RenderBackend`, over [`PlanPresenterState`]/
/// [`PlanUiEvent`] instead of the check pipeline's own. Kept as a separate
/// trait (rather than genericizing the check presenter's) so neither side's
/// backends are forced to know about the other's states.
pub(crate) trait PlanRenderBackend: Send {
    fn apply(&mut self, state: &PlanPresenterState, event: &PlanUiEvent);
    fn tick(&mut self, state: &PlanPresenterState);
    fn teardown(&mut self, state: &PlanPresenterState);
    fn tick_interval(&self) -> std::time::Duration;
}

/// The chosen backend plus whether it **owns the final record** — mirrors
/// `crate::checks::presenter::Backend` exactly: only the inline TUI flushes
/// the requirement tree to scrollback itself, so `plan::run_with_planner`
/// must print the plain final record to stdout on every other path (the
/// heartbeat backend, or no live presenter at all).
pub(crate) struct Backend {
    pub backend: Box<dyn PlanRenderBackend>,
    pub owns_record: bool,
}

/// Choose the backend for a real `multi plan` run: the inline TUI when stdout
/// is a TTY (falling back to the heartbeat if terminal setup fails),
/// otherwise the stderr heartbeat — identical policy to
/// `crate::checks::presenter::select_backend`.
pub(crate) fn select_backend(color: bool) -> Backend {
    let stderr_is_tty = std::io::stderr().is_terminal();
    if std::io::stdout().is_terminal()
        && let Ok(backend) = PlanInlineTuiBackend::new(color)
    {
        return Backend {
            backend: Box::new(backend),
            owns_record: true,
        };
    }
    Backend {
        backend: Box::new(PlanHeartbeatBackend::new(stderr_is_tty)),
        owns_record: false,
    }
}

/// A cheap, cloneable, fire-and-forget handle a caller uses to tell the plan
/// presenter about a [`PlanUiEvent`] — the plan-side counterpart of
/// `crate::checks::executor::ProgressSink`, except over the actor's mailbox
/// (an ordinary `tell`) rather than a bounded channel, since these events (one
/// per check-lifecycle milestone, plus one per agent turn) are far lower
/// volume than a token-level stream. Threaded, optionally, through
/// `plan::run_with_planner` → `plan::process_check` → `PlanRequest` →
/// `AgentPlanner` — `None` everywhere planning has no live UI (every existing
/// orchestration test).
#[derive(Clone, Debug)]
pub(crate) struct PlanEventSink(ActorRef<PlanPresenterActor>);

impl PlanEventSink {
    pub(crate) fn new(actor: ActorRef<PlanPresenterActor>) -> Self {
        Self(actor)
    }

    /// Fire-and-forget: a dead presenter (the actor panicked, or was never
    /// spawned) silently drops the event rather than propagating an error —
    /// planning must never be affected by presentation (see the module docs).
    pub(crate) async fn send(&self, event: PlanUiEvent) {
        let _ = self.0.tell(event).await;
    }
}

/// A live presenter's sink plus whether its backend owns the final terminal
/// record — the plan-side counterpart of
/// `crate::checks::presenter::Backend`'s `owns_record` flag, bundled with the
/// sink itself since `plan::run_with_planner` always needs both together (and
/// never one without the other). `None` (not this type at all) in every
/// orchestration test in `checks::plan` — see [`select_backend`]'s docs.
pub(crate) struct Presentation {
    pub sink: PlanEventSink,
    pub owns_record: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kameo::actor::Spawn;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A backend that records the [`PlanUiEvent`]s it is handed, for an
    /// end-to-end actor test — mirrors
    /// `crate::checks::presenter::tests::RecordingBackend`.
    struct RecordingBackend {
        events: Arc<Mutex<Vec<PlanUiEvent>>>,
    }

    impl RecordingBackend {
        fn new() -> (Self, Arc<Mutex<Vec<PlanUiEvent>>>) {
            let events = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    events: events.clone(),
                },
                events,
            )
        }
    }

    impl PlanRenderBackend for RecordingBackend {
        fn apply(&mut self, _state: &PlanPresenterState, event: &PlanUiEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
        fn tick(&mut self, _state: &PlanPresenterState) {}
        fn teardown(&mut self, _state: &PlanPresenterState) {}
        fn tick_interval(&self) -> Duration {
            Duration::from_secs(3600)
        }
    }

    /// End-to-end through the actor: events drive state and reach the
    /// backend, and a graceful stop runs teardown.
    #[tokio::test]
    async fn actor_folds_events_and_records_them() {
        let (backend, events) = RecordingBackend::new();
        let presenter = PlanPresenterActor::spawn(PlanPresenterActor::new(
            Box::new(backend),
            "test-model".into(),
        ));
        let sink = PlanEventSink::new(presenter.clone());

        sink.send(PlanUiEvent::Queued {
            id: 0,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        })
        .await;
        sink.send(PlanUiEvent::DiscoveryComplete { total_checks: 1 })
            .await;
        sink.send(PlanUiEvent::Fresh {
            id: 0,
            truncated: false,
        })
        .await;

        presenter.stop_gracefully().await.unwrap();
        presenter.wait_for_shutdown().await;

        let recorded = events.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        assert!(matches!(recorded[0], PlanUiEvent::Queued { id: 0, .. }));
        assert!(matches!(recorded[2], PlanUiEvent::Fresh { id: 0, .. }));
    }
}
