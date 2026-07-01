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

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use crate::checks::model::{CheckId, CheckOutcome, RequirementOutcome, Verdict};

use super::UiEvent;

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
}

/// The presenter's whole view-model, mutated by [`PresenterState::apply`].
///
/// [`PresenterState::apply`]: PresenterState::apply
pub(crate) struct PresenterState {
    /// When the run began (for the total-elapsed header counter).
    pub run_started: Instant,
    /// Total checks to expect; `None` until `DiscoveryComplete`.
    pub total: Option<usize>,
    /// Whether discovery finished streaming (all rows now exist).
    pub discovery_complete: bool,
    /// Every check, keyed by id. A `BTreeMap` so iteration is in id order, which
    /// is `(req_index, declaration)` order — matching the canonical report.
    pub rows: BTreeMap<CheckId, CheckRow>,
}

impl PresenterState {
    /// A fresh state stamped with the run's start instant.
    pub(crate) fn new() -> Self {
        Self {
            run_started: Instant::now(),
            total: None,
            discovery_complete: false,
            rows: BTreeMap::new(),
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
                    },
                );
            }
            UiEvent::CheckStarted { id } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = CheckState::Running;
                    row.started = Some(Instant::now());
                }
            }
            UiEvent::CheckRetrying { id, attempt } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = CheckState::Retrying(*attempt);
                }
            }
            UiEvent::CheckSettled { id, outcome } => {
                if let Some(row) = self.rows.get_mut(id) {
                    row.state = CheckState::Settled(outcome.verdict);
                    row.outcome = Some(outcome.clone());
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
        let mut s = PresenterState::new();
        s.apply(&UiEvent::CheckQueued {
            id: 0,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
        assert_eq!(s.pending(), 1);
        assert_eq!(s.running(), 0);

        s.apply(&UiEvent::CheckStarted { id: 0 });
        assert_eq!(s.running(), 1);
        assert_eq!(s.pending(), 0);

        // A retry pulls it out of Running and back into the pending tally...
        s.apply(&UiEvent::CheckRetrying { id: 0, attempt: 1 });
        assert_eq!(s.running(), 0);
        assert_eq!(s.pending(), 1);

        // ...then a fresh start counts it as running again (no double-count).
        s.apply(&UiEvent::CheckStarted { id: 0 });
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
        let mut s = PresenterState::new();
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
}
