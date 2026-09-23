//! The non-TTY backend for `multi plan` (MULTI-1829) — the plan-side
//! counterpart of `crate::checks::presenter::heartbeat`, with one behavioral
//! difference the ticket asks for explicitly: **one line per transition to a
//! terminal state**, in addition to the periodic summary line. `multi
//! check`'s heartbeat never emits a per-event line (only the periodic
//! summary) because every check eventually settles and a per-check line would
//! flood CI logs; `multi plan` re-runs an agent for only the (usually few)
//! stale checks, so a line per terminal outcome (`fresh`/`planned`/
//! `agent-only`/`error`) stays readable and gives CI logs a durable per-check
//! trail — never a per-progress-event line (that would flood logs exactly the
//! way `multi check`'s design avoids).

use std::io::Write;
use std::time::Duration;

use super::PlanRenderBackend;
use super::PlanUiEvent;
use super::state::PlanPresenterState;
use crate::checks::plan::agent_reason_str;
use crate::checks::presenter::format::human_elapsed;

/// How often the periodic summary line refreshes — identical cadence to
/// `crate::checks::presenter::heartbeat::HEARTBEAT_INTERVAL`.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub(crate) struct PlanHeartbeatBackend {
    stderr_is_tty: bool,
    line_pending: bool,
}

impl PlanHeartbeatBackend {
    pub(crate) fn new(stderr_is_tty: bool) -> Self {
        Self {
            stderr_is_tty,
            line_pending: false,
        }
    }

    /// The periodic summary line, e.g. `[multi plan] 4/12 checks settled ·
    /// fresh 8 · re-planned 2 · agent-only 1 · errors 1 · 3m12s ·
    /// jev-latest`. `None` before there's anything meaningful to report.
    fn summary(&self, state: &PlanPresenterState) -> Option<String> {
        let total = state.total?;
        if total == 0 {
            return None;
        }
        let (fresh, replanned, agent_only, errors) = state.outcome_tallies();
        let elapsed = human_elapsed(state.run_started.elapsed());
        let truncated = state.truncated_count();
        let truncated_suffix = if truncated > 0 {
            format!(" · {truncated} truncated")
        } else {
            String::new()
        };
        Some(format!(
            "[multi plan] {}/{total} checks settled · fresh {fresh} · re-planned {replanned} · agent-only {agent_only} · errors {errors} · {elapsed} · {}{truncated_suffix}",
            state.done(),
            state.model,
        ))
    }

    fn emit(&mut self, line: &str) {
        let mut err = std::io::stderr().lock();
        if self.stderr_is_tty {
            let _ = write!(err, "\r{line}\x1b[K");
        } else {
            let _ = writeln!(err, "{line}");
        }
        let _ = err.flush();
        self.line_pending = true;
    }

    /// One line for a check that just reached a terminal state, or `None` for
    /// anything else — see the module docs on why only terminal states get a
    /// per-event line.
    fn transition_line(state: &PlanPresenterState, event: &PlanUiEvent) -> Option<String> {
        let (id, tag) = match event {
            PlanUiEvent::Fresh { id, .. } => (*id, "fresh".to_string()),
            PlanUiEvent::Planned { id, verdict, .. } => (
                *id,
                format!("planned (jev, {})", if *verdict { "pass" } else { "fail" }),
            ),
            PlanUiEvent::AgentOnly {
                id,
                reason,
                verdict,
                ..
            } => (
                *id,
                format!(
                    "agent-only ({}, {})",
                    agent_reason_str(*reason),
                    if *verdict { "pass" } else { "fail" }
                ),
            ),
            PlanUiEvent::Error { id, message } => (*id, format!("error: {message}")),
            _ => return None,
        };
        let row = state.rows.get(&id)?;
        Some(format!(
            "[multi plan] {} :: {} — {tag}",
            row.req_title, row.check_title
        ))
    }
}

impl PlanRenderBackend for PlanHeartbeatBackend {
    fn apply(&mut self, state: &PlanPresenterState, event: &PlanUiEvent) {
        if let PlanUiEvent::Log(line) = event {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err, "{line}");
            let _ = err.flush();
            return;
        }
        if let Some(line) = Self::transition_line(state, event) {
            self.emit(&line);
        }
    }

    fn tick(&mut self, state: &PlanPresenterState) {
        if let Some(line) = self.summary(state) {
            self.emit(&line);
        }
    }

    fn teardown(
        &mut self,
        _state: &PlanPresenterState,
        _final_record: Option<&super::FinalRecord>,
    ) {
        // This backend never owns the terminal record (`owns_record` is
        // always `false` — see `select_backend`): `plan::run_with_planner`
        // prints the plain final record to stdout itself (from the same
        // `FinalRecord` value, when there is one), so there is nothing for
        // this backend's own teardown to render — just clear the in-place
        // progress line so it never collides with that print.
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
    use crate::checks::jev::plan_file::AgentReason;

    fn queued(state: &mut PlanPresenterState, id: usize) {
        state.apply(&PlanUiEvent::Queued {
            id,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
    }

    #[test]
    fn no_summary_until_total_is_known() {
        let backend = PlanHeartbeatBackend::new(false);
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0);
        assert!(backend.summary(&state).is_none());

        state.apply(&PlanUiEvent::DiscoveryComplete { total_checks: 1 });
        let line = backend.summary(&state).expect("known total");
        assert!(
            line.starts_with("[multi plan] 0/1 checks settled"),
            "{line}"
        );
        assert!(line.ends_with("m"), "{line}");
    }

    #[test]
    fn empty_suite_emits_no_summary() {
        let backend = PlanHeartbeatBackend::new(true);
        let mut state = PlanPresenterState::new("m".into());
        state.apply(&PlanUiEvent::DiscoveryComplete { total_checks: 0 });
        assert!(backend.summary(&state).is_none());
    }

    #[test]
    fn summary_reflects_all_four_terminal_tallies() {
        let backend = PlanHeartbeatBackend::new(false);
        let mut state = PlanPresenterState::new("m".into());
        for id in 0..4 {
            queued(&mut state, id);
        }
        state.apply(&PlanUiEvent::DiscoveryComplete { total_checks: 4 });
        state.apply(&PlanUiEvent::Fresh {
            id: 0,
            truncated: false,
        });
        state.apply(&PlanUiEvent::Planned {
            id: 1,
            verdict: true,
            truncated: false,
        });
        state.apply(&PlanUiEvent::AgentOnly {
            id: 2,
            reason: AgentReason::JevUncertain,
            verdict: true,
            truncated: false,
        });
        state.apply(&PlanUiEvent::Error {
            id: 3,
            message: "boom".into(),
        });

        let line = backend.summary(&state).unwrap();
        assert!(line.contains("4/4 checks settled"), "{line}");
        assert!(line.contains("fresh 1"), "{line}");
        assert!(line.contains("re-planned 1"), "{line}");
        assert!(line.contains("agent-only 1"), "{line}");
        assert!(line.contains("errors 1"), "{line}");
    }

    #[test]
    fn summary_appends_the_truncated_count_when_positive() {
        let backend = PlanHeartbeatBackend::new(false);
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0);
        state.apply(&PlanUiEvent::DiscoveryComplete { total_checks: 1 });
        state.apply(&PlanUiEvent::Fresh {
            id: 0,
            truncated: true,
        });
        let line = backend.summary(&state).unwrap();
        assert!(line.contains("1 truncated"), "{line}");
    }

    /// MULTI-1829 acceptance: a transition line is emitted for every terminal
    /// state, never for a non-terminal one (no per-progress-event lines).
    #[test]
    fn transition_line_fires_only_for_terminal_states() {
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0);
        state.apply(&PlanUiEvent::Verifying { id: 0 });
        assert!(
            PlanHeartbeatBackend::transition_line(&state, &PlanUiEvent::Verifying { id: 0 })
                .is_none()
        );
        assert!(
            PlanHeartbeatBackend::transition_line(
                &state,
                &PlanUiEvent::Progress {
                    id: 0,
                    attempt: 1,
                    turn: 1,
                    max_turns: 30,
                    activity: None,
                }
            )
            .is_none()
        );

        state.apply(&PlanUiEvent::Fresh {
            id: 0,
            truncated: false,
        });
        let line = PlanHeartbeatBackend::transition_line(
            &state,
            &PlanUiEvent::Fresh {
                id: 0,
                truncated: false,
            },
        )
        .expect("Fresh is terminal");
        assert!(line.contains("fresh"), "{line}");
        assert!(line.contains("R :: c"), "{line}");
    }

    #[test]
    fn transition_line_for_agent_only_names_the_reason_and_verdict() {
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0);
        let event = PlanUiEvent::AgentOnly {
            id: 0,
            reason: AgentReason::ControlFailed,
            verdict: false,
            truncated: false,
        };
        state.apply(&event);
        let line = PlanHeartbeatBackend::transition_line(&state, &event).unwrap();
        assert!(line.contains("agent-only (control failed, fail)"), "{line}");
    }

    #[test]
    fn transition_line_for_error_carries_the_message() {
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0);
        let event = PlanUiEvent::Error {
            id: 0,
            message: "no MultiTool.toml manifest found".into(),
        };
        state.apply(&event);
        let line = PlanHeartbeatBackend::transition_line(&state, &event).unwrap();
        assert!(
            line.contains("error: no MultiTool.toml manifest found"),
            "{line}"
        );
    }
}
