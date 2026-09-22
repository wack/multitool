//! The non-TTY backend (MULTI-1369).
//!
//! When stdout is not a terminal (pipe, redirect, CI), there is no live UI — but
//! a silent run still reads as a hang. [`HeartbeatBackend`] closes that gap with
//! a compact progress line on **stderr** at a slow cadence, leaving stdout
//! reserved for the reporting actor's byte-for-byte report.
//!
//! * If **stderr is a TTY**, overwrite one tidy line in place with `\r`, and clear
//!   it on teardown so nothing lingers before the final report.
//! * If **stderr is not a TTY** (the CI-log case), append a fresh line each
//!   interval so the log preserves progression.
//!
//! The cadence is slow (`HEARTBEAT_INTERVAL`) on purpose: short runs — including
//! the millisecond fake-executor tests — emit nothing at all.

use std::io::Write;
use std::time::Duration;

use crate::checks::model::decided_by_summary;

use super::UiEvent;
use super::backend::RenderBackend;
use super::format::human_elapsed;
use super::state::PresenterState;

/// How often the heartbeat line refreshes. Slow enough that fast runs stay quiet.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// Emits a periodic one-line progress heartbeat to stderr.
pub(crate) struct HeartbeatBackend {
    /// Whether stderr is a TTY: in-place (`\r`) vs append-a-line.
    stderr_is_tty: bool,
    /// Whether an in-place line is currently on screen (so teardown can clear it).
    line_pending: bool,
}

impl HeartbeatBackend {
    pub(crate) fn new(stderr_is_tty: bool) -> Self {
        Self {
            stderr_is_tty,
            line_pending: false,
        }
    }

    /// The heartbeat text, e.g. `[multi] 7/12 checks complete · 4 running
    /// (turn 3/30, turn 12/30) · 3m12s · claude-sonnet-4-6 · 2 cached · 1 jev
    /// · 4 agent`. Returns `None` before there's anything meaningful to
    /// report.
    ///
    /// The parenthetical lists the turn of each currently-running check
    /// (MULTI-1828), in row order — the CI-log substitute for the inline
    /// TUI's per-row `turn N/max` — but only for the events already folded
    /// into `state`; this method itself never emits per-event lines (only
    /// `tick`, on the slow heartbeat cadence, does).
    ///
    /// The trailing decider-count segment (MULTI-1827) appears the instant
    /// the *first* non-agent-decided check settles (rather than waiting for
    /// the whole run to finish) — same liveness principle as everything else
    /// in this line — and never at all in the default build, where every
    /// check is `DecidedBy::Agent` and [`decided_by_summary`] returns `None`.
    fn line(&self, state: &PresenterState) -> Option<String> {
        let total = state.total?;
        if total == 0 {
            return None;
        }
        let elapsed = human_elapsed(state.run_started.elapsed());
        let turns = state.running_turns();
        let progress = if turns.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = turns
                .iter()
                .map(|(turn, max_turns)| format!("turn {turn}/{max_turns}"))
                .collect();
            format!(" ({})", parts.join(", "))
        };
        let mut line = format!(
            "[multi] {}/{total} checks complete · {} running{progress} · {elapsed} · {}",
            state.done(),
            state.running(),
            state.model,
        );
        let (cached, jev, agent) = state.decided_by_tallies();
        if let Some(summary) = decided_by_summary(cached, jev, agent) {
            line.push_str(&format!(" · {summary}"));
        }
        Some(line)
    }

    fn emit(&mut self, state: &PresenterState) {
        let Some(line) = self.line(state) else {
            return;
        };
        let mut err = std::io::stderr().lock();
        if self.stderr_is_tty {
            // Overwrite in place; clear to end-of-line in case the prior line was
            // longer (`\x1b[K`).
            let _ = write!(err, "\r{line}\x1b[K");
        } else {
            let _ = writeln!(err, "{line}");
        }
        let _ = err.flush();
        self.line_pending = true;
    }
}

impl RenderBackend for HeartbeatBackend {
    fn apply(&mut self, _state: &PresenterState, event: &UiEvent) {
        // Heartbeat is otherwise purely time-driven; events only update shared
        // state. Routed log lines are the exception: the presenter is now
        // `tracing`'s sole sink for the run (see the module docs), so if we
        // don't re-emit them here they simply vanish. Stderr, not stdout, to
        // honor this backend's own invariant that stdout stays reserved for the
        // reporting actor's byte-for-byte report.
        if let UiEvent::Log(line) = event {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err, "{line}");
            let _ = err.flush();
        }
    }

    fn tick(&mut self, state: &PresenterState) {
        self.emit(state);
    }

    fn teardown(&mut self, _state: &PresenterState) {
        // Clear the in-place line so it never collides with the final report.
        if self.stderr_is_tty && self.line_pending {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[K");
            let _ = err.flush();
        }
        self.line_pending = false;
    }

    fn tick_interval(&self) -> Duration {
        HEARTBEAT_INTERVAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::model::{CheckOutcome, DecidedBy, Verdict};

    fn queued(state: &mut PresenterState, id: usize) {
        state.apply(&UiEvent::CheckQueued {
            id,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
    }

    #[test]
    fn no_line_until_total_is_known() {
        let backend = HeartbeatBackend::new(false);
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        // Total unknown ⇒ nothing to print yet.
        assert!(backend.line(&state).is_none());

        state.apply(&UiEvent::DiscoveryComplete { total_checks: 2 });
        let line = backend.line(&state).expect("line once total is known");
        assert!(line.starts_with("[multi] 0/2 checks complete"), "{line}");
        assert!(line.ends_with("test-model"), "{line}");
    }

    #[test]
    fn line_reflects_done_and_running_counts() {
        let backend = HeartbeatBackend::new(false);
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        queued(&mut state, 1);
        state.apply(&UiEvent::DiscoveryComplete { total_checks: 2 });
        state.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        state.apply(&UiEvent::CheckSettled {
            id: 1,
            outcome: CheckOutcome {
                title: "c".into(),
                verdict: Verdict::Satisfied,
                evidence: None,
                decided_by: DecidedBy::Agent,
            },
        });
        let line = backend.line(&state).unwrap();
        assert!(line.contains("1/2 checks complete"), "{line}");
        assert!(line.contains("1 running"), "{line}");
    }

    #[test]
    fn empty_suite_emits_nothing() {
        let backend = HeartbeatBackend::new(true);
        let mut state = PresenterState::new("test-model".into());
        state.apply(&UiEvent::DiscoveryComplete { total_checks: 0 });
        assert!(backend.line(&state).is_none());
    }

    /// MULTI-1828 acceptance: the heartbeat line includes the turn of each
    /// running check (never the activity text — that would flood CI logs).
    #[test]
    fn line_includes_the_turn_of_each_running_check() {
        let backend = HeartbeatBackend::new(false);
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        queued(&mut state, 1);
        state.apply(&UiEvent::DiscoveryComplete { total_checks: 2 });
        state.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        state.apply(&UiEvent::CheckStarted { id: 1, attempt: 1 });
        state.apply(&UiEvent::CheckProgress {
            id: 0,
            attempt: 1,
            turn: 3,
            max_turns: 30,
            activity: Some("Read src/auth/sign.rs".into()),
        });
        state.apply(&UiEvent::CheckProgress {
            id: 1,
            attempt: 1,
            turn: 12,
            max_turns: 30,
            activity: None,
        });

        let line = backend.line(&state).unwrap();
        assert!(line.contains("turn 3/30"), "{line}");
        assert!(line.contains("turn 12/30"), "{line}");
        // Never the activity text itself — only the turn.
        assert!(!line.contains("sign.rs"), "{line}");
    }

    /// A running check with no progress yet (no `TurnStart` observed) is
    /// simply omitted rather than padded with a placeholder.
    #[test]
    fn line_omits_turn_for_a_running_check_with_no_progress_yet() {
        let backend = HeartbeatBackend::new(false);
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        state.apply(&UiEvent::DiscoveryComplete { total_checks: 1 });
        state.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });

        let line = backend.line(&state).unwrap();
        assert!(!line.contains("turn"), "{line}");
    }

    fn settle(state: &mut PresenterState, id: usize, decided_by: DecidedBy) {
        state.apply(&UiEvent::CheckSettled {
            id,
            outcome: CheckOutcome {
                title: "c".into(),
                verdict: Verdict::Satisfied,
                evidence: None,
                decided_by,
            },
        });
    }

    /// MULTI-1827 acceptance: an all-`Agent` run (the default build, always)
    /// carries no decider-count segment at all — the heartbeat line is
    /// exactly what it was before this ticket.
    #[test]
    fn line_has_no_decider_summary_when_every_check_is_agent_decided() {
        let backend = HeartbeatBackend::new(false);
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        state.apply(&UiEvent::DiscoveryComplete { total_checks: 1 });
        settle(&mut state, 0, DecidedBy::Agent);

        let line = backend.line(&state).unwrap();
        assert!(!line.contains("cached"), "{line}");
        assert!(!line.contains("jev"), "{line}");
    }

    /// MULTI-1827 acceptance: the decider-count segment appears the instant
    /// the *first* non-agent-decided check settles, not only once the whole
    /// run finishes, and reflects `N cached · N jev · N agent`.
    #[test]
    fn line_gains_decider_summary_as_soon_as_a_non_agent_check_settles() {
        let backend = HeartbeatBackend::new(false);
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        queued(&mut state, 1);
        state.apply(&UiEvent::DiscoveryComplete { total_checks: 2 });

        // Still all pending/running: no summary yet.
        assert!(!backend.line(&state).unwrap().contains("cached"));

        settle(&mut state, 0, DecidedBy::Cached);
        let line = backend.line(&state).unwrap();
        assert!(line.contains("1 cached · 0 jev · 0 agent"), "{line}");

        settle(&mut state, 1, DecidedBy::Jev);
        let line = backend.line(&state).unwrap();
        assert!(line.contains("1 cached · 1 jev · 0 agent"), "{line}");
    }
}
