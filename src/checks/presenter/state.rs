//! The presenter's view-model: the single source of truth a [`RenderBackend`]
//! renders from (MULTI-1369).
//!
//! [`Message<UiEvent>`] mutates this state; [`Message<Tick>`] then asks the
//! backend to render *from* it. Keeping all derived counts as cheap scans over
//! [`PresenterState::rows`] (recomputed per event, never per frame) sidesteps the
//! increment/decrement bookkeeping bugs an incremental tally would invite — a
//! check that retries moves Running → Retrying → Running again, and a from-scratch
//! scan is always correct regardless of the path taken.
//!
//! [`RenderBackend`]: super::backend::RenderBackend
//! [`Message<UiEvent>`]: super::PresenterActor
//! [`Message<Tick>`]: super::PresenterActor

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use crate::checks::model::{CheckId, CheckOutcome, RequirementOutcome, Verdict};

use super::UiEvent;

/// How many recent log lines the live view keeps for at-a-glance context. Full
/// history is never lost — every line is *also* flushed straight to permanent
/// scrollback (see `InlineTuiBackend::flush_log_line`) — so this only bounds the
/// ephemeral in-viewport pane.
const RECENT_LOGS_CAP: usize = 3;

/// Where a single check is in its lifecycle, as seen by the presenter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CheckState {
    /// Discovered + enqueued, not yet running (no permit acquired).
    Queued,
    /// An agent is currently running for this check.
    Running,
    /// A prior attempt finished without a verdict; awaiting its re-run. The
    /// `u32` is the just-finished attempt number.
    Retrying(u32),
    /// Reached a terminal verdict.
    Settled(Verdict),
}

impl CheckState {
    fn is_settled(&self) -> bool {
        matches!(self, CheckState::Settled(_))
    }
}

/// One row in the requirement→check tree: a single check's live state.
#[derive(Clone, Debug)]
pub(crate) struct CheckRow {
    /// Which requirement (declaration order) this check rolls up into.
    pub req_index: usize,
    /// The owning requirement's title (for the tree parent + the record).
    pub req_title: String,
    /// This check's title (the tree leaf label).
    pub check_title: String,
    /// The check's current lifecycle state.
    pub state: CheckState,
    /// When the current attempt began running (for the climbing elapsed timer).
    pub started: Option<Instant>,
    /// The reconciled outcome, set once settled (carries evidence for the record).
    pub outcome: Option<CheckOutcome>,
    /// The attempt currently running (1-based), set by the most recent
    /// `CheckStarted` (MULTI-1828); `0` before the first attempt starts.
    /// `CheckProgress` is accepted only when it names this exact attempt —
    /// the progress-forwarding task for a finished attempt is *aborted*, not
    /// joined (see `execution::run_one`), so a queued update from it is never
    /// guaranteed to stop arriving before the next attempt's own events do.
    pub attempt: u32,
    /// The current attempt's most recent in-flight progress (MULTI-1828).
    /// `None` until the first `CheckProgress` for this attempt arrives;
    /// cleared on every `CheckStarted`, retry, and settle so a stale
    /// turn/activity never survives past the attempt it described.
    pub progress: Option<CheckProgress>,
}

/// One check's live progress, folded from [`UiEvent::CheckProgress`].
/// Display-only: never affects verdicts, retries, or reporting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CheckProgress {
    /// The turn the agent is currently on.
    pub turn: u32,
    /// The executor's configured turn ceiling.
    pub max_turns: u32,
    /// A short, root-relative rendering of the most recent allowlisted tool
    /// call, when there is one to show.
    pub activity: Option<String>,
}

/// The presenter's whole view-model, mutated by [`PresenterState::apply`].
///
/// [`PresenterState::apply`]: PresenterState::apply
pub(crate) struct PresenterState {
    /// The model running this suite's checks, shown in the header so a run
    /// against a non-default provider/model doesn't look identical to one
    /// against the default.
    pub model: String,
    /// When the run began (for the total-elapsed header counter).
    pub run_started: Instant,
    /// Total checks to expect; `None` until `DiscoveryComplete`.
    pub total: Option<usize>,
    /// Whether discovery finished streaming (all rows now exist).
    pub discovery_complete: bool,
    /// Every check, keyed by id. A `BTreeMap` so iteration is in id order, which
    /// is `(req_index, declaration)` order — matching the canonical report.
    pub rows: BTreeMap<CheckId, CheckRow>,
    /// The last [`RECENT_LOGS_CAP`] routed log lines, oldest first.
    pub recent_logs: VecDeque<String>,
}

impl PresenterState {
    /// A fresh state stamped with the run's start instant.
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

    /// Fold one [`UiEvent`] into the view-model.
    pub(crate) fn apply(&mut self, event: &UiEvent) {
        match event {
            UiEvent::DiscoveryComplete { total_checks } => {
                self.total = Some(*total_checks);
                self.discovery_complete = true;
            }
            UiEvent::CheckQueued {
                id,
                req_index,
                req_title,
                check_title,
            } => {
                self.rows.insert(
                    *id,
                    CheckRow {
                        req_index: *req_index,
                        req_title: req_title.clone(),
                        check_title: check_title.clone(),
                        state: CheckState::Queued,
                        started: None,
                        outcome: None,
                        attempt: 0,
                        progress: None,
                    },
                );
            }
            UiEvent::CheckStarted { id, attempt } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = CheckState::Running;
                    row.started = Some(Instant::now());
                    row.attempt = *attempt;
                    // A fresh attempt starts silent (MULTI-1828): any
                    // progress left over from a previous attempt — or a
                    // stray update that arrived while this one was between
                    // attempts — no longer describes anything.
                    row.progress = None;
                }
            }
            UiEvent::CheckRetrying { id, attempt } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = CheckState::Retrying(*attempt);
                    // A fresh attempt starts silent (MULTI-1828): the prior
                    // attempt's turn/activity no longer describes anything.
                    row.progress = None;
                }
            }
            UiEvent::CheckSettled { id, outcome } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = CheckState::Settled(outcome.verdict);
                    row.outcome = Some(outcome.clone());
                    // Nothing left to show once settled (MULTI-1828).
                    row.progress = None;
                }
            }
            UiEvent::CheckProgress {
                id,
                attempt,
                turn,
                max_turns,
                activity,
            } => {
                // Accepted only for a row that is exactly `Running` *and* on
                // exactly the attempt this update names (MULTI-1828 code
                // review). Progress forwarding is best-effort and
                // un-ordered relative to the check's own lifecycle events —
                // `execution::run_one` aborts its forwarder task rather than
                // waiting for it to drain, so a queued update from a
                // finished attempt can still arrive after `CheckRetrying` or
                // even after the *next* attempt's `CheckStarted`. Neither
                // check alone is enough: `state == Running` alone would
                // still accept a same-numbered update that outlived a
                // retry-then-restart, and `attempt` alone would still accept
                // one that arrives while merely `Retrying`.
                if let Some(row) = self.rows.get_mut(id)
                    && matches!(row.state, CheckState::Running)
                    && row.attempt == *attempt
                {
                    row.progress = Some(CheckProgress {
                        turn: *turn,
                        max_turns: *max_turns,
                        activity: activity.clone(),
                    });
                }
            }
            UiEvent::Log(line) => {
                self.recent_logs.push_back(line.clone());
                if self.recent_logs.len() > RECENT_LOGS_CAP {
                    self.recent_logs.pop_front();
                }
            }
        }
    }

    /// How many checks have settled (the gauge numerator).
    pub(crate) fn done(&self) -> usize {
        self.rows.values().filter(|r| r.state.is_settled()).count()
    }

    /// Per-verdict tally over settled checks: `(satisfied, failed, errored)`.
    pub(crate) fn verdict_tallies(&self) -> (usize, usize, usize) {
        let mut sat = 0;
        let mut failed = 0;
        let mut errored = 0;
        for row in self.rows.values() {
            match row.state {
                CheckState::Settled(Verdict::Satisfied) => sat += 1,
                CheckState::Settled(Verdict::Failed) => failed += 1,
                CheckState::Settled(Verdict::Errored) => errored += 1,
                _ => {}
            }
        }
        (sat, failed, errored)
    }

    /// How many checks are running right now.
    pub(crate) fn running(&self) -> usize {
        self.rows
            .values()
            .filter(|r| matches!(r.state, CheckState::Running))
            .count()
    }

    /// How many checks are pending (queued or between retry attempts).
    pub(crate) fn pending(&self) -> usize {
        self.rows
            .values()
            .filter(|r| matches!(r.state, CheckState::Queued | CheckState::Retrying(_)))
            .count()
    }

    /// The `(turn, max_turns)` of every currently-running check that has
    /// reported at least one turn, in id order — the heartbeat backend's
    /// "turn of each running check" (MULTI-1828). A running check with no
    /// progress yet (no turn observed) is simply omitted rather than padded
    /// with a placeholder.
    pub(crate) fn running_turns(&self) -> Vec<(u32, u32)> {
        self.rows
            .values()
            .filter(|r| matches!(r.state, CheckState::Running))
            .filter_map(|r| r.progress.as_ref().map(|p| (p.turn, p.max_turns)))
            .collect()
    }

    /// The set of distinct `req_index`es, in ascending (declaration) order.
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
    pub(crate) fn requirement_rows(&self, req_index: usize) -> Vec<&CheckRow> {
        self.rows
            .values()
            .filter(|r| r.req_index == req_index)
            .collect()
    }

    /// Whether every check of `req_index` has settled (so the requirement can be
    /// flushed to the record). Only meaningful once discovery has completed, since
    /// otherwise more checks for this requirement could still arrive.
    pub(crate) fn requirement_complete(&self, req_index: usize) -> bool {
        let rows = self.requirement_rows(req_index);
        !rows.is_empty() && rows.iter().all(|r| r.state.is_settled())
    }

    /// Reconstruct a requirement's [`RequirementOutcome`] from its settled rows,
    /// using the **same** AND-aggregation the reporting actor uses — so the
    /// presenter's record can never diverge from the canonical verdict. The
    /// filepath is irrelevant to rendering and left empty.
    pub(crate) fn requirement_outcome(&self, req_index: usize) -> Option<RequirementOutcome> {
        let rows = self.requirement_rows(req_index);
        let first = rows.first()?;
        let title = first.req_title.clone();
        let check_outcomes: Vec<CheckOutcome> =
            rows.iter().filter_map(|r| r.outcome.clone()).collect();
        Some(RequirementOutcome::aggregate(
            title,
            PathBuf::new(),
            check_outcomes,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled(verdict: Verdict) -> CheckOutcome {
        CheckOutcome {
            title: "c".into(),
            verdict,
            evidence: None,
        }
    }

    #[test]
    fn tallies_recompute_from_rows_across_a_retry() {
        let mut s = PresenterState::new("test-model".into());
        s.apply(&UiEvent::CheckQueued {
            id: 0,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
        assert_eq!(s.pending(), 1);
        assert_eq!(s.running(), 0);

        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        assert_eq!(s.running(), 1);
        assert_eq!(s.pending(), 0);

        // A retry pulls it out of Running and back into the pending tally...
        s.apply(&UiEvent::CheckRetrying { id: 0, attempt: 1 });
        assert_eq!(s.running(), 0);
        assert_eq!(s.pending(), 1);

        // ...then a fresh start counts it as running again (no double-count).
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 2 });
        assert_eq!(s.running(), 1);

        s.apply(&UiEvent::CheckSettled {
            id: 0,
            outcome: settled(Verdict::Satisfied),
        });
        assert_eq!(s.done(), 1);
        assert_eq!(s.running(), 0);
        assert_eq!(s.verdict_tallies(), (1, 0, 0));
    }

    #[test]
    fn requirement_completion_and_outcome_use_shared_aggregation() {
        let mut s = PresenterState::new("test-model".into());
        for (id, title) in [(0, "a"), (1, "b")] {
            s.apply(&UiEvent::CheckQueued {
                id,
                req_index: 0,
                req_title: "Req".into(),
                check_title: title.into(),
            });
        }
        s.apply(&UiEvent::DiscoveryComplete { total_checks: 2 });

        // Not complete until both settle.
        s.apply(&UiEvent::CheckSettled {
            id: 0,
            outcome: settled(Verdict::Satisfied),
        });
        assert!(!s.requirement_complete(0));

        s.apply(&UiEvent::CheckSettled {
            id: 1,
            outcome: settled(Verdict::Failed),
        });
        assert!(s.requirement_complete(0));

        // AND-aggregation: one Failed check fails the requirement.
        let outcome = s.requirement_outcome(0).unwrap();
        assert_eq!(outcome.title, "Req");
        assert!(!outcome.satisfied);
        assert_eq!(outcome.check_outcomes.len(), 2);
    }

    fn progress(attempt: u32, turn: u32, max_turns: u32, activity: Option<&str>) -> UiEvent {
        UiEvent::CheckProgress {
            id: 0,
            attempt,
            turn,
            max_turns,
            activity: activity.map(str::to_string),
        }
    }

    fn queued(s: &mut PresenterState, id: CheckId) {
        s.apply(&UiEvent::CheckQueued {
            id,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
    }

    /// MULTI-1828 acceptance: progress updates a running row.
    #[test]
    fn progress_updates_a_running_row() {
        let mut s = PresenterState::new("test-model".into());
        queued(&mut s, 0);
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        assert!(s.rows.get(&0).unwrap().progress.is_none());

        s.apply(&progress(1, 1, 30, None));
        let p = s.rows.get(&0).unwrap().progress.clone().unwrap();
        assert_eq!(p.turn, 1);
        assert_eq!(p.max_turns, 30);
        assert_eq!(p.activity, None);

        s.apply(&progress(1, 1, 30, Some("Read src/auth/sign.rs")));
        let p = s.rows.get(&0).unwrap().progress.clone().unwrap();
        assert_eq!(p.activity.as_deref(), Some("Read src/auth/sign.rs"));
        assert_eq!(s.running_turns(), vec![(1, 30)]);
    }

    /// MULTI-1828 acceptance: progress is cleared on retry.
    #[test]
    fn progress_is_cleared_on_retry() {
        let mut s = PresenterState::new("test-model".into());
        queued(&mut s, 0);
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        s.apply(&progress(1, 3, 30, Some("Grep \"sign_jwt\"")));
        assert!(s.rows.get(&0).unwrap().progress.is_some());

        s.apply(&UiEvent::CheckRetrying { id: 0, attempt: 1 });
        assert!(s.rows.get(&0).unwrap().progress.is_none());
        assert!(s.running_turns().is_empty());
    }

    /// MULTI-1828 acceptance: progress is cleared on settle.
    #[test]
    fn progress_is_cleared_on_settle() {
        let mut s = PresenterState::new("test-model".into());
        queued(&mut s, 0);
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        s.apply(&progress(1, 5, 30, Some("Glob \"**/*.rs\"")));
        assert!(s.rows.get(&0).unwrap().progress.is_some());

        s.apply(&UiEvent::CheckSettled {
            id: 0,
            outcome: settled(Verdict::Satisfied),
        });
        assert!(s.rows.get(&0).unwrap().progress.is_none());
    }

    /// MULTI-1828 acceptance: events for an unknown id are ignored.
    #[test]
    fn progress_for_an_unknown_id_is_ignored() {
        let mut s = PresenterState::new("test-model".into());
        // No `CheckQueued` for id 0 at all.
        s.apply(&progress(1, 1, 30, Some("Read x")));
        assert!(s.rows.is_empty());
    }

    /// MULTI-1828 acceptance: a straggling event for an already-settled id
    /// is ignored (it must not resurrect a turn/activity display for a row
    /// that's already showing its final verdict).
    #[test]
    fn progress_for_a_settled_id_is_ignored() {
        let mut s = PresenterState::new("test-model".into());
        queued(&mut s, 0);
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        s.apply(&UiEvent::CheckSettled {
            id: 0,
            outcome: settled(Verdict::Satisfied),
        });

        s.apply(&progress(1, 9, 30, Some("Read late.rs")));
        assert!(s.rows.get(&0).unwrap().progress.is_none());
    }

    /// MULTI-1828 code review blocker: a late progress update for the
    /// attempt that just finished must be ignored while the row sits
    /// `Retrying`, awaiting its next attempt — even though `attempt` still
    /// matches, since the forwarder for a finished attempt is aborted, not
    /// joined, so it can still deliver a queued update after `CheckRetrying`.
    #[test]
    fn late_progress_while_retrying_is_ignored() {
        let mut s = PresenterState::new("test-model".into());
        queued(&mut s, 0);
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        s.apply(&UiEvent::CheckRetrying { id: 0, attempt: 1 });

        // A straggling update from attempt 1, arriving after the retry.
        s.apply(&progress(1, 7, 30, Some("Read straggler.rs")));
        assert!(s.rows.get(&0).unwrap().progress.is_none());
    }

    /// MULTI-1828 code review blocker: a late progress update from a
    /// *previous* attempt must be ignored once the *next* attempt has
    /// started running — it must not be misattributed to the new attempt.
    #[test]
    fn progress_from_a_previous_attempt_after_the_next_checkstarted_is_ignored() {
        let mut s = PresenterState::new("test-model".into());
        queued(&mut s, 0);
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        s.apply(&UiEvent::CheckRetrying { id: 0, attempt: 1 });
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 2 });

        // A straggling attempt-1 update, arriving after attempt 2 started.
        s.apply(&progress(1, 4, 30, Some("Read stale.rs")));
        assert!(s.rows.get(&0).unwrap().progress.is_none());

        // Attempt 2's own progress is accepted normally.
        s.apply(&progress(2, 1, 30, Some("Read fresh.rs")));
        let p = s.rows.get(&0).unwrap().progress.clone().unwrap();
        assert_eq!(p.activity.as_deref(), Some("Read fresh.rs"));
    }

    /// MULTI-1828 code review blocker: `CheckStarted` itself clears any
    /// progress still sitting on the row (defense in depth alongside the
    /// attempt-number check above — even if a stale update *did* somehow
    /// carry the new attempt's number, a just-started attempt has not run
    /// long enough to have produced any progress of its own yet).
    #[test]
    fn checkstarted_clears_any_leftover_progress() {
        let mut s = PresenterState::new("test-model".into());
        queued(&mut s, 0);
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        s.apply(&progress(1, 5, 30, Some("Read mid-flight.rs")));
        assert!(s.rows.get(&0).unwrap().progress.is_some());

        s.apply(&UiEvent::CheckRetrying { id: 0, attempt: 1 });
        s.apply(&UiEvent::CheckStarted { id: 0, attempt: 2 });
        assert!(s.rows.get(&0).unwrap().progress.is_none());
    }
}
