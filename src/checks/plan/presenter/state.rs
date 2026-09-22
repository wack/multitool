//! The plan presenter's view-model (MULTI-1829) — the plan-side counterpart
//! of `crate::checks::presenter::state`. Same shape, same reasoning (derived
//! counts are cheap from-scratch scans over [`PlanPresenterState::rows`],
//! never an incremental tally), different row states: [`PlanRowState`] is the
//! ticket's own state machine (`Queued → Verifying → Fresh | Stale{reason} →
//! Planning → Calibrating → Planned | AgentOnly | Error`), not `multi
//! check`'s.

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use crate::checks::jev::plan_file::AgentReason;
use crate::checks::model::CheckId;

use super::PlanUiEvent;

/// How many recent log lines the live view keeps — mirrors
/// `crate::checks::presenter::state::RECENT_LOGS_CAP`.
const RECENT_LOGS_CAP: usize = 3;

/// Why a check's frozen plan (or lack of one) is stale — the ticket's closed
/// set, in the exact order it lists them. Distinguishing [`StaleReason::New`]
/// from [`StaleReason::PromptChanged`] needs a positional lookup that ignores
/// the prompt hash (see `plan::find_existing_entry`); the other two mirror
/// [`crate::checks::jev::replay::Freshness`]'s two non-fresh variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StaleReason {
    /// No plan entry exists at this check's position at all.
    New,
    /// An entry exists at this position, but its stored `prompt_xxh64` no
    /// longer matches the check's current title/prompt.
    PromptChanged,
    /// [`crate::checks::jev::replay::Freshness::ReadsStale`]: some call's
    /// content changed, but not the set of files it depends on.
    FilesChanged,
    /// [`crate::checks::jev::replay::Freshness::DiscoveryStale`]: the set of
    /// files a call depends on is no longer what the plan assumed.
    FileSetChanged,
    /// `--force` — the cache was bypassed unconditionally.
    Forced,
}

impl StaleReason {
    /// The row tag text — plain, never color-only (the ticket: "respect the
    /// existing no-color path").
    pub(crate) fn tag(self) -> &'static str {
        match self {
            StaleReason::New => "new",
            StaleReason::PromptChanged => "prompt changed",
            StaleReason::FilesChanged => "files changed",
            StaleReason::FileSetChanged => "file set changed",
            StaleReason::Forced => "forced",
        }
    }
}

/// Where a single check is in `multi plan`'s lifecycle, as seen by the
/// presenter — the ticket's row states verbatim.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PlanRowState {
    /// Discovered, shown immediately (before any cache lookup runs).
    Queued,
    /// Replaying the existing plan's tool calls and comparing checksums.
    Verifying,
    /// Checksums matched: reused, no agent run.
    Fresh,
    /// Stale (or missing, or forced): about to be (re-)planned. The reason
    /// lives on [`PlanRow::stale_reason`], not here, so it survives the
    /// transition into `Planning`/`Calibrating` without being repeated on
    /// every event.
    Stale,
    /// The agent is running this (1-based) attempt.
    Planning { attempt: u32 },
    /// A prior attempt finished without a verdict; the just-finished attempt
    /// number, awaiting its retry.
    Retrying { attempt: u32 },
    /// Asking Jev to reproduce the agent's verdict (and, usually, to reject
    /// the empty-evidence negative control).
    Calibrating,
    /// Terminal: `decider = "jev"`.
    Planned { verdict: bool },
    /// Terminal: `decider = "agent"`, with why.
    AgentOnly { reason: AgentReason, verdict: bool },
    /// Terminal: the agent never reported a verdict (or errored), or the
    /// check's file was refused (no manifest-derived root).
    Error,
}

impl PlanRowState {
    /// Whether this state is one of the ticket's terminal states — gates the
    /// gauge's `done` numerator and the footer's per-outcome tallies.
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self,
            PlanRowState::Fresh
                | PlanRowState::Planned { .. }
                | PlanRowState::AgentOnly { .. }
                | PlanRowState::Error
        )
    }
}

/// One check's live progress — identical shape to
/// `crate::checks::presenter::state::CheckProgress`, folded from
/// [`PlanUiEvent::Progress`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlanProgress {
    pub turn: u32,
    pub max_turns: u32,
    pub activity: Option<String>,
}

/// One row in the requirement→check tree: a single check's live plan state.
#[derive(Clone, Debug)]
pub(crate) struct PlanRow {
    pub req_index: usize,
    pub req_title: String,
    pub check_title: String,
    pub state: PlanRowState,
    /// Set by `Stale` and retained through `Planning`/`Retrying`/
    /// `Calibrating` — `None` only for a row that is (or has only ever been)
    /// `Queued`/`Verifying`/`Fresh`.
    pub stale_reason: Option<StaleReason>,
    /// When the current attempt began running (for the elapsed timer) — set
    /// by `Planning`, mirroring `CheckRow::started`.
    pub started: Option<Instant>,
    /// The attempt currently running (1-based); `0` before `Planning` first
    /// fires. Guards a stale `Progress` update exactly as
    /// `crate::checks::presenter::state` does for `multi check`.
    pub attempt: u32,
    pub progress: Option<PlanProgress>,
    /// Whether this row's terminal entry carries at least one
    /// `PlanCall::Truncated` call — set only by `Fresh`/`Planned`/
    /// `AgentOnly` (never by a non-terminal event), so the footer's "N
    /// truncated" is always a from-scratch scan over terminal rows alone.
    pub truncated: bool,
    /// Set by `Error` — the diagnostic shown in place of a verdict.
    pub error_message: Option<String>,
}

/// The plan presenter's whole view-model, mutated by
/// [`PlanPresenterState::apply`]. This is the **live** view only — the
/// authoritative final record (MULTI-1829 code review) is a separate value,
/// [`super::FinalRecord`], built directly from orchestration ground truth and
/// delivered independently of this state — see its own docs.
pub(crate) struct PlanPresenterState {
    pub model: String,
    pub run_started: Instant,
    pub total: Option<usize>,
    pub discovery_complete: bool,
    pub rows: BTreeMap<CheckId, PlanRow>,
    pub recent_logs: VecDeque<String>,
}

impl PlanPresenterState {
    pub(crate) fn new(model: String) -> Self {
        Self {
            model,
            run_started: Instant::now(),
            total: None,
            discovery_complete: false,
            rows: BTreeMap::new(),
            recent_logs: VecDeque::new(),
        }
    }

    /// Fold one [`PlanUiEvent`] into the view-model.
    pub(crate) fn apply(&mut self, event: &PlanUiEvent) {
        match event {
            PlanUiEvent::DiscoveryComplete { total_checks } => {
                self.total = Some(*total_checks);
                self.discovery_complete = true;
            }
            PlanUiEvent::Queued {
                id,
                req_index,
                req_title,
                check_title,
            } => {
                self.rows.insert(
                    *id,
                    PlanRow {
                        req_index: *req_index,
                        req_title: req_title.clone(),
                        check_title: check_title.clone(),
                        state: PlanRowState::Queued,
                        stale_reason: None,
                        started: None,
                        attempt: 0,
                        progress: None,
                        truncated: false,
                        error_message: None,
                    },
                );
            }
            PlanUiEvent::Verifying { id } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = PlanRowState::Verifying;
                }
            }
            PlanUiEvent::Fresh { id, truncated } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = PlanRowState::Fresh;
                    row.truncated = *truncated;
                    row.progress = None;
                }
            }
            PlanUiEvent::Stale { id, reason } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = PlanRowState::Stale;
                    row.stale_reason = Some(*reason);
                }
            }
            PlanUiEvent::Planning { id, attempt } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = PlanRowState::Planning { attempt: *attempt };
                    row.attempt = *attempt;
                    row.started = Some(Instant::now());
                    // A fresh attempt starts silent — mirrors
                    // `CheckState::Running`'s `CheckStarted` handling.
                    row.progress = None;
                }
            }
            PlanUiEvent::Retrying { id, attempt } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = PlanRowState::Retrying { attempt: *attempt };
                    row.progress = None;
                }
            }
            PlanUiEvent::Progress {
                id,
                attempt,
                turn,
                max_turns,
                activity,
            } => {
                // Same stale-attempt protection as `multi check`'s presenter
                // (MULTI-1828 code review): only a row that is exactly
                // `Planning` *and* on exactly the named attempt accepts the
                // update — see `crate::checks::presenter::state`'s docs for
                // why both checks are needed together.
                if let Some(row) = self.rows.get_mut(id)
                    && matches!(row.state, PlanRowState::Planning { .. })
                    && row.attempt == *attempt
                {
                    row.progress = Some(PlanProgress {
                        turn: *turn,
                        max_turns: *max_turns,
                        activity: activity.clone(),
                    });
                }
            }
            PlanUiEvent::Calibrating { id } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = PlanRowState::Calibrating;
                    row.progress = None;
                }
            }
            PlanUiEvent::Planned {
                id,
                verdict,
                truncated,
            } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = PlanRowState::Planned { verdict: *verdict };
                    row.truncated = *truncated;
                    row.progress = None;
                }
            }
            PlanUiEvent::AgentOnly {
                id,
                reason,
                verdict,
                truncated,
            } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = PlanRowState::AgentOnly {
                        reason: *reason,
                        verdict: *verdict,
                    };
                    row.truncated = *truncated;
                    row.progress = None;
                }
            }
            PlanUiEvent::Error { id, message } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = PlanRowState::Error;
                    row.error_message = Some(message.clone());
                    row.progress = None;
                }
            }
            PlanUiEvent::Log(line) => {
                self.recent_logs.push_back(line.clone());
                if self.recent_logs.len() > RECENT_LOGS_CAP {
                    self.recent_logs.pop_front();
                }
            }
        }
    }

    /// How many checks have settled into a terminal state (the gauge
    /// numerator).
    pub(crate) fn done(&self) -> usize {
        self.rows.values().filter(|r| r.state.is_terminal()).count()
    }

    /// The footer's four terminal tallies: `(fresh, re-planned, agent-only,
    /// errors)`.
    pub(crate) fn outcome_tallies(&self) -> (usize, usize, usize, usize) {
        let mut fresh = 0;
        let mut replanned = 0;
        let mut agent_only = 0;
        let mut errors = 0;
        for row in self.rows.values() {
            match row.state {
                PlanRowState::Fresh => fresh += 1,
                PlanRowState::Planned { .. } => replanned += 1,
                PlanRowState::AgentOnly { .. } => agent_only += 1,
                PlanRowState::Error => errors += 1,
                _ => {}
            }
        }
        (fresh, replanned, agent_only, errors)
    }

    /// How many terminal rows carry at least one truncated call — the
    /// footer's "N truncated" and the final record's repeated count.
    pub(crate) fn truncated_count(&self) -> usize {
        self.rows.values().filter(|r| r.truncated).count()
    }

    /// Whether any row is currently mid-flight (not `Queued` and not yet
    /// terminal) — used to order the live tree so active requirements sort
    /// first, mirroring `crate::checks::presenter::inline`'s ordering.
    pub(crate) fn is_active(state: &PlanRowState) -> bool {
        matches!(
            state,
            PlanRowState::Verifying
                | PlanRowState::Stale
                | PlanRowState::Planning { .. }
                | PlanRowState::Retrying { .. }
                | PlanRowState::Calibrating
        )
    }

    /// The set of distinct `req_index`es, ascending.
    pub(crate) fn requirement_indices(&self) -> Vec<usize> {
        let mut seen = Vec::new();
        for row in self.rows.values() {
            if !seen.contains(&row.req_index) {
                seen.push(row.req_index);
            }
        }
        seen.sort_unstable();
        seen
    }

    /// The rows of one requirement, in id (declaration) order.
    pub(crate) fn requirement_rows(&self, req_index: usize) -> Vec<(&CheckId, &PlanRow)> {
        self.rows
            .iter()
            .filter(|(_, r)| r.req_index == req_index)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(s: &mut PlanPresenterState, id: CheckId) {
        s.apply(&PlanUiEvent::Queued {
            id,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
    }

    #[test]
    fn queued_row_starts_in_the_queued_state() {
        let mut s = PlanPresenterState::new("m".into());
        queued(&mut s, 0);
        assert_eq!(s.rows.get(&0).unwrap().state, PlanRowState::Queued);
        assert_eq!(s.done(), 0);
    }

    #[test]
    fn verifying_transitions_a_queued_row() {
        let mut s = PlanPresenterState::new("m".into());
        queued(&mut s, 0);
        s.apply(&PlanUiEvent::Verifying { id: 0 });
        assert_eq!(s.rows.get(&0).unwrap().state, PlanRowState::Verifying);
    }

    #[test]
    fn fresh_is_terminal_and_carries_the_truncated_flag() {
        let mut s = PlanPresenterState::new("m".into());
        queued(&mut s, 0);
        s.apply(&PlanUiEvent::Verifying { id: 0 });
        s.apply(&PlanUiEvent::Fresh {
            id: 0,
            truncated: true,
        });
        let row = s.rows.get(&0).unwrap();
        assert_eq!(row.state, PlanRowState::Fresh);
        assert!(row.truncated);
        assert_eq!(s.done(), 1);
        assert_eq!(s.outcome_tallies(), (1, 0, 0, 0));
        assert_eq!(s.truncated_count(), 1);
    }

    /// Every stale reason the ticket names, each round-tripping through
    /// `apply` and surviving on the row (not the state) through the
    /// transition into `Planning`.
    #[test]
    fn every_stale_reason_is_recorded_and_survives_into_planning() {
        for reason in [
            StaleReason::New,
            StaleReason::PromptChanged,
            StaleReason::FilesChanged,
            StaleReason::FileSetChanged,
            StaleReason::Forced,
        ] {
            let mut s = PlanPresenterState::new("m".into());
            queued(&mut s, 0);
            s.apply(&PlanUiEvent::Verifying { id: 0 });
            s.apply(&PlanUiEvent::Stale { id: 0, reason });
            let row = s.rows.get(&0).unwrap();
            assert_eq!(row.state, PlanRowState::Stale);
            assert_eq!(row.stale_reason, Some(reason));

            s.apply(&PlanUiEvent::Planning { id: 0, attempt: 1 });
            let row = s.rows.get(&0).unwrap();
            assert_eq!(row.state, PlanRowState::Planning { attempt: 1 });
            assert_eq!(
                row.stale_reason,
                Some(reason),
                "the stale reason must survive into Planning: {reason:?}"
            );
        }
    }

    #[test]
    fn retrying_clears_progress_but_keeps_the_stale_reason() {
        let mut s = PlanPresenterState::new("m".into());
        queued(&mut s, 0);
        s.apply(&PlanUiEvent::Stale {
            id: 0,
            reason: StaleReason::New,
        });
        s.apply(&PlanUiEvent::Planning { id: 0, attempt: 1 });
        s.apply(&PlanUiEvent::Progress {
            id: 0,
            attempt: 1,
            turn: 2,
            max_turns: 30,
            activity: Some("Read x".into()),
        });
        assert!(s.rows.get(&0).unwrap().progress.is_some());

        s.apply(&PlanUiEvent::Retrying { id: 0, attempt: 1 });
        let row = s.rows.get(&0).unwrap();
        assert_eq!(row.state, PlanRowState::Retrying { attempt: 1 });
        assert!(row.progress.is_none());
        assert_eq!(row.stale_reason, Some(StaleReason::New));
    }

    #[test]
    fn progress_is_rejected_for_a_mismatched_attempt() {
        let mut s = PlanPresenterState::new("m".into());
        queued(&mut s, 0);
        s.apply(&PlanUiEvent::Stale {
            id: 0,
            reason: StaleReason::New,
        });
        s.apply(&PlanUiEvent::Planning { id: 0, attempt: 1 });
        s.apply(&PlanUiEvent::Retrying { id: 0, attempt: 1 });
        // A straggling update from the just-finished attempt, arriving while
        // the row sits `Retrying` (not `Planning`) — must be ignored.
        s.apply(&PlanUiEvent::Progress {
            id: 0,
            attempt: 1,
            turn: 5,
            max_turns: 30,
            activity: None,
        });
        assert!(s.rows.get(&0).unwrap().progress.is_none());

        s.apply(&PlanUiEvent::Planning { id: 0, attempt: 2 });
        // Attempt 1's straggler must not be misattributed to attempt 2.
        s.apply(&PlanUiEvent::Progress {
            id: 0,
            attempt: 1,
            turn: 9,
            max_turns: 30,
            activity: None,
        });
        assert!(s.rows.get(&0).unwrap().progress.is_none());

        // Attempt 2's own progress is accepted normally.
        s.apply(&PlanUiEvent::Progress {
            id: 0,
            attempt: 2,
            turn: 1,
            max_turns: 30,
            activity: Some("Grep x".into()),
        });
        assert!(s.rows.get(&0).unwrap().progress.is_some());
    }

    #[test]
    fn calibrating_clears_progress_and_keeps_the_stale_reason() {
        let mut s = PlanPresenterState::new("m".into());
        queued(&mut s, 0);
        s.apply(&PlanUiEvent::Stale {
            id: 0,
            reason: StaleReason::FilesChanged,
        });
        s.apply(&PlanUiEvent::Planning { id: 0, attempt: 1 });
        s.apply(&PlanUiEvent::Progress {
            id: 0,
            attempt: 1,
            turn: 2,
            max_turns: 30,
            activity: None,
        });
        s.apply(&PlanUiEvent::Calibrating { id: 0 });
        let row = s.rows.get(&0).unwrap();
        assert_eq!(row.state, PlanRowState::Calibrating);
        assert!(row.progress.is_none());
        assert_eq!(row.stale_reason, Some(StaleReason::FilesChanged));
    }

    #[test]
    fn planned_is_terminal_jev_decided() {
        let mut s = PlanPresenterState::new("m".into());
        queued(&mut s, 0);
        s.apply(&PlanUiEvent::Stale {
            id: 0,
            reason: StaleReason::New,
        });
        s.apply(&PlanUiEvent::Planning { id: 0, attempt: 1 });
        s.apply(&PlanUiEvent::Calibrating { id: 0 });
        s.apply(&PlanUiEvent::Planned {
            id: 0,
            verdict: true,
            truncated: false,
        });
        let row = s.rows.get(&0).unwrap();
        assert_eq!(row.state, PlanRowState::Planned { verdict: true });
        assert_eq!(s.done(), 1);
        assert_eq!(s.outcome_tallies(), (0, 1, 0, 0));
    }

    #[test]
    fn agent_only_is_terminal_for_every_reason() {
        for reason in [
            AgentReason::JevDisagreed,
            AgentReason::JevUncertain,
            AgentReason::ControlFailed,
            AgentReason::OverBudget,
            AgentReason::NoToolCalls,
            AgentReason::TruncatedDiscovery,
        ] {
            let mut s = PlanPresenterState::new("m".into());
            queued(&mut s, 0);
            s.apply(&PlanUiEvent::Stale {
                id: 0,
                reason: StaleReason::New,
            });
            s.apply(&PlanUiEvent::Planning { id: 0, attempt: 1 });
            s.apply(&PlanUiEvent::AgentOnly {
                id: 0,
                reason,
                verdict: false,
                truncated: reason == AgentReason::TruncatedDiscovery,
            });
            let row = s.rows.get(&0).unwrap();
            assert_eq!(
                row.state,
                PlanRowState::AgentOnly {
                    reason,
                    verdict: false
                }
            );
            assert_eq!(s.outcome_tallies(), (0, 0, 1, 0));
        }
    }

    #[test]
    fn error_is_terminal_and_carries_a_message() {
        let mut s = PlanPresenterState::new("m".into());
        queued(&mut s, 0);
        s.apply(&PlanUiEvent::Error {
            id: 0,
            message: "agent never reported".into(),
        });
        let row = s.rows.get(&0).unwrap();
        assert_eq!(row.state, PlanRowState::Error);
        assert_eq!(row.error_message.as_deref(), Some("agent never reported"));
        assert_eq!(s.outcome_tallies(), (0, 0, 0, 1));
        assert_eq!(s.done(), 1);
    }

    #[test]
    fn a_fully_fresh_run_reports_zero_agent_runs() {
        // MULTI-1829 acceptance: everything fresh renders every row Fresh —
        // and, since `Planning`/`Calibrating` are never applied, the
        // presenter's own view agrees no agent ever ran.
        let mut s = PlanPresenterState::new("m".into());
        for id in 0..3 {
            queued(&mut s, id);
            s.apply(&PlanUiEvent::Verifying { id });
            s.apply(&PlanUiEvent::Fresh {
                id,
                truncated: false,
            });
        }
        s.apply(&PlanUiEvent::DiscoveryComplete { total_checks: 3 });

        assert!(s.rows.values().all(|r| r.state == PlanRowState::Fresh));
        assert_eq!(s.outcome_tallies(), (3, 0, 0, 0));
        assert_eq!(s.done(), 3);
        assert!(
            s.rows
                .values()
                .all(|r| r.attempt == 0 && r.progress.is_none()),
            "no row was ever touched by Planning/Progress"
        );
    }

    #[test]
    fn recent_logs_are_capped() {
        let mut s = PlanPresenterState::new("m".into());
        for i in 0..5 {
            s.apply(&PlanUiEvent::Log(format!("line {i}")));
        }
        assert_eq!(s.recent_logs.len(), RECENT_LOGS_CAP);
        assert_eq!(s.recent_logs.back().unwrap(), "line 4");
    }

    #[test]
    fn events_for_an_unknown_id_are_ignored() {
        let mut s = PlanPresenterState::new("m".into());
        s.apply(&PlanUiEvent::Fresh {
            id: 0,
            truncated: false,
        });
        assert!(s.rows.is_empty());
    }
}
