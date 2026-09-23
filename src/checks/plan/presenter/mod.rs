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
//! ## Delivery is best-effort, never blocking (MULTI-1829 code review)
//!
//! `multi check`'s own presenter sends (`checks::execution::run_one` et al.)
//! `.await` a `tell` over the actor's default **bounded** (64-deep) mailbox —
//! meaning a slow/stalled presenter can, in principle, back-pressure the check
//! pipeline. That is pre-existing `multi check` behavior this ticket does not
//! touch. `multi plan` cannot inherit it: [`process_check`](super::process_check)/
//! [`plan_it`](super::plan_it) and [`AgentPlanner`](super::planner::AgentPlanner)
//! gate cache verification, agent start/retry, calibration, and the final
//! plan write, so [`PlanEventSink::send`] is a plain, **non-async**,
//! best-effort call (`tell(..).try_send()`): it can never block the caller,
//! and a full mailbox or a dead actor just drops the event (logged at
//! `debug`). Every send stays a synchronous call within the same task that
//! already owns that check's row, never a detached `tokio::spawn`, so one
//! row's own events can never reorder relative to each other — only
//! reordering *across* independently-spawned tasks would need that
//! precaution, and nothing here does that.
//!
//! Because delivery is lossy, the **final record** (the requirement tree with
//! every check's terminal state, its stale reason, and the written plan
//! files) is never reconstructed from accumulated [`PlanUiEvent`]s — see
//! [`FinalRecord`]'s own docs for how it bypasses the mailbox entirely.
//!
//! [`plan::mod`]: super

mod heartbeat;
mod inline;
mod record;
mod state;

use std::io::IsTerminal;
use std::time::Duration;

use kameo::Actor;
use kameo::actor::{ActorRef, WeakActorRef};
use kameo::error::ActorStopReason;
use kameo::message::{Context, Message};

use crate::checks::jev::plan_file::AgentReason;
use crate::checks::model::CheckId;
use crate::terminal::{LogRouteGuard, route_logs};

use heartbeat::PlanHeartbeatBackend;
use inline::PlanInlineTuiBackend;
pub(crate) use record::{FinalCheck, FinalOutcome, FinalRecord, FinalRecordSlot, FinalRequirement};
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
    /// Where `plan::run_with_planner` deposits the run's one, authoritative
    /// [`FinalRecord`] — read directly here in [`Actor::on_stop`], bypassing
    /// the (lossy, best-effort) event mailbox entirely. See [`FinalRecord`]'s
    /// docs.
    final_record: FinalRecordSlot,
    ticker: Option<tokio::task::JoinHandle<()>>,
    log_pump: Option<tokio::task::JoinHandle<()>>,
    log_route: Option<LogRouteGuard>,
}

impl PlanPresenterActor {
    pub(crate) fn new(
        backend: Box<dyn PlanRenderBackend>,
        model: String,
        final_record: FinalRecordSlot,
    ) -> Self {
        Self {
            state: PlanPresenterState::new(model),
            backend,
            final_record,
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
        self.backend.teardown(&self.state, self.final_record.get());
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
    /// Final cleanup. `final_record` is the run's one authoritative
    /// [`FinalRecord`], when `plan::run_with_planner` got far enough to build
    /// one (`None` only when the run aborted before then — see
    /// [`FinalRecord`]'s docs) — the inline backend renders **from this**,
    /// never from `state`, so a dropped/backlogged live event can never
    /// corrupt the flushed record.
    fn teardown(&mut self, state: &PlanPresenterState, final_record: Option<&FinalRecord>);
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

    /// Fire-and-forget, **non-blocking** (MULTI-1829 code review): a `try_send`
    /// over the actor's mailbox, never a `.await`. A dead presenter (the
    /// actor panicked, was never spawned, or has already been stopped) or a
    /// momentarily full mailbox both just drop the event (logged at
    /// `debug!`) rather than propagating an error or blocking the caller —
    /// planning must never be delayed or affected by presentation (see the
    /// module docs' "Delivery is best-effort, never blocking" section).
    /// Called synchronously from within whichever task already owns the
    /// row's lifecycle, never from a detached `tokio::spawn`, so one row's
    /// own events are never reordered relative to each other.
    pub(crate) fn send(&self, event: PlanUiEvent) {
        if let Err(err) = self.0.tell(event).try_send() {
            tracing::debug!(?err, "dropped a plan presenter event");
        }
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
    /// The side-channel `plan::run_with_planner` deposits this run's
    /// [`FinalRecord`] into, bypassing `sink`'s lossy mailbox — see
    /// [`FinalRecord`]'s docs.
    pub final_record: FinalRecordSlot,
}

/// How long [`shutdown`] waits for the plan presenter to stop gracefully
/// before giving up. All planning work (including writing every
/// `.check-plan.toml`) is complete by the time this runs — see
/// `plan::run`'s call site — so this bound exists purely to stop a stuck
/// presenter (e.g. blocked writing to a stdout pipe nobody is reading) from
/// hanging the whole `multi plan` process after there is nothing left to do.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Stop `actor`, bounding the wait so a stuck presenter can never hang
/// `multi plan` after planning has already finished (MULTI-1829 code
/// review). On timeout, kills the actor outright rather than waiting
/// indefinitely.
///
/// Terminal/cursor restoration does not depend on this succeeding within the
/// timeout: [`crate::checks::presenter::install_terminal_guards`] (installed
/// by [`inline::PlanInlineTuiBackend::new`]) independently restores the
/// cursor on a panic or SIGINT, and [`ActorRef::kill`]'s own contract still
/// runs the actor's `on_stop` (and therefore the inline backend's own
/// teardown) even after a kill. As a third, defense-in-depth layer — since
/// none of the above is a *guarantee* against a shutdown that hangs for a
/// reason other than those two (e.g. the actor genuinely deadlocked
/// mid-`tick`, never reaching an await point `kill` can interrupt) — a timed
/// out shutdown also shows the cursor directly here, unconditionally: cheap,
/// idempotent, and harmless even for the heartbeat backend, which never hid
/// it in the first place.
///
/// Never returns an error and never touches the caller's own result — see
/// `plan::run`'s call site, which runs this after already having its
/// `run_with_planner` result in hand.
pub(crate) async fn shutdown(actor: &ActorRef<PlanPresenterActor>) {
    let graceful = async {
        let _ = actor.stop_gracefully().await;
        actor.wait_for_shutdown().await;
    };
    if tokio::time::timeout(SHUTDOWN_TIMEOUT, graceful)
        .await
        .is_err()
    {
        tracing::warn!(
            timeout = ?SHUTDOWN_TIMEOUT,
            "plan presenter did not shut down in time; killing it",
        );
        actor.kill();
        force_show_cursor();
    }
}

/// Best-effort, idempotent cursor restoration — see [`shutdown`]'s docs.
fn force_show_cursor() {
    use ratatui::crossterm::{cursor, execute};
    let _ = execute!(std::io::stdout(), cursor::Show);
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
        fn teardown(&mut self, _state: &PlanPresenterState, _final_record: Option<&FinalRecord>) {}
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
            FinalRecordSlot::new(),
        ));
        let sink = PlanEventSink::new(presenter.clone());

        sink.send(PlanUiEvent::Queued {
            id: 0,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
        sink.send(PlanUiEvent::DiscoveryComplete { total_checks: 1 });
        sink.send(PlanUiEvent::Fresh {
            id: 0,
            truncated: false,
        });

        presenter.stop_gracefully().await.unwrap();
        presenter.wait_for_shutdown().await;

        let recorded = events.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        assert!(matches!(recorded[0], PlanUiEvent::Queued { id: 0, .. }));
        assert!(matches!(recorded[2], PlanUiEvent::Fresh { id: 0, .. }));
    }

    /// MULTI-1829 code review (blocking item 1): a dead presenter must never
    /// affect the sender — `send` after the actor has already stopped is a
    /// synchronous, non-blocking no-op.
    #[tokio::test]
    async fn send_after_the_actor_is_stopped_does_not_block_or_panic() {
        let (backend, _events) = RecordingBackend::new();
        let presenter = PlanPresenterActor::spawn(PlanPresenterActor::new(
            Box::new(backend),
            "test-model".into(),
            FinalRecordSlot::new(),
        ));
        presenter.stop_gracefully().await.unwrap();
        presenter.wait_for_shutdown().await;

        let sink = PlanEventSink::new(presenter);
        // Synchronous — if this were still `.await`ing a bounded mailbox
        // send, a dead actor's closed channel would still resolve promptly;
        // the real regression this guards is a *live but stalled* actor,
        // covered by `plan::tests::planning_completes_when_the_presenter_actor_is_dead`
        // at the orchestration level. Here we just assert it never panics.
        sink.send(PlanUiEvent::Queued {
            id: 0,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
    }
}
