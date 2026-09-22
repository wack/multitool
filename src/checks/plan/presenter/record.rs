//! The final record (MULTI-1829 code review: blocking item 4).
//!
//! [`PlanEventSink::send`](super::PlanEventSink::send) is best-effort and can
//! drop events under backpressure — fine for the *live* view (the next
//! update supersedes a missed one), but wrong for the record `multi plan`
//! flushes to scrollback (or stdout) at the very end: that record must be
//! **exactly** what was planned and written, never a possibly-incomplete
//! reconstruction from whichever [`PlanUiEvent`](super::PlanUiEvent)s
//! happened to arrive.
//!
//! So [`FinalRecord`] is built **once**, in `plan::run_with_planner`,
//! directly from the orchestration's own ground truth (`processed`/
//! `refused`/`written_paths` — see `plan::build_final_record`) — never
//! accumulated from applied events. It reaches the presenter through
//! [`FinalRecordSlot`], a plain shared cell set once by the orchestration and
//! read once by [`PlanPresenterActor::on_stop`](super::PlanPresenterActor),
//! entirely bypassing the actor's mailbox: even if every single
//! [`PlanUiEvent`](super::PlanUiEvent) for a run was dropped, the final
//! record delivered at teardown is still complete and correct, because its
//! delivery was never subject to mailbox capacity or actor liveness in the
//! first place (the slot is set synchronously from the same task that
//! finishes `run_with_planner`, and reading it later never blocks — an
//! unset slot just reads back `None`).
//!
//! Both backends render from the *same* value: the inline TUI (via
//! [`PlanPresenterActor::on_stop`](super::PlanPresenterActor)) flushes it to
//! scrollback, and `plan::run_with_planner` itself prints it as plain text to
//! stdout whenever no TTY-owning backend already did (including when there
//! is no live presenter at all, or its actor died, or its shutdown timed
//! out) — see `plan::print_plain_record`.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::checks::jev::plan_file::AgentReason;

use super::StaleReason;

/// One check's terminal outcome for the final record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FinalOutcome {
    /// Checksums matched: the plan was reused, no agent ran.
    Fresh,
    /// `decider = "jev"`.
    Planned { verdict: bool },
    /// `decider = "agent"`, with why.
    AgentOnly { reason: AgentReason, verdict: bool },
    /// The agent never reported a verdict (or errored), or the check's file
    /// was refused (no manifest-derived root).
    Error { message: String },
}

/// One check's row in the final record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FinalCheck {
    pub title: String,
    pub outcome: FinalOutcome,
    /// Why this check was re-planned — `Some` only for [`FinalOutcome::Planned`]/
    /// [`FinalOutcome::AgentOnly`] (the ticket: "the stale reason for every
    /// re-planned check"); always `None` for `Fresh`/`Error`, which were
    /// never (re-)planned this run.
    pub stale_reason: Option<StaleReason>,
    /// Whether this check's final entry carries at least one truncated call
    /// — see `plan::calls_have_truncated`.
    pub truncated: bool,
}

/// One requirement's checks in the final record, in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FinalRequirement {
    pub title: String,
    pub checks: Vec<FinalCheck>,
}

/// The complete, authoritative record of one `multi plan` run — see the
/// module docs. Built exactly once by `plan::build_final_record`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FinalRecord {
    /// In presenter-tree order (`plan::assign_req_indices`'s traversal
    /// order) — refused and plannable requirements interleaved by that
    /// shared key, exactly like the live tree.
    pub requirements: Vec<FinalRequirement>,
    /// How many checks' final entry carries at least one truncated call —
    /// repeated here so both backends' final record can show it without
    /// recomputing it from `requirements` (and to stay byte-identical with
    /// `plan::RunReport::truncated_count`, computed from the same source).
    pub truncated_count: usize,
    /// Every check considered this run, including refused-file checks —
    /// mirrors `plan::RunReport::total_checks`.
    pub total_checks: usize,
    /// Every `.check-plan.toml` written this run, sorted.
    pub written_paths: Vec<PathBuf>,
}

/// A write-once, read-many shared cell carrying this run's [`FinalRecord`] —
/// see the module docs for why this, rather than a [`PlanUiEvent`](super::PlanUiEvent),
/// is how the record reaches the presenter. Cheap to clone (an `Arc` around a
/// `OnceLock`); every clone reads back the same value once any one of them
/// sets it.
#[derive(Clone, Default)]
pub(crate) struct FinalRecordSlot(Arc<OnceLock<FinalRecord>>);

impl FinalRecordSlot {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Set the final record. Idempotent by construction (a
    /// [`OnceLock`] ignores a second `set`) — `plan::run_with_planner` only
    /// ever calls this once, at most, per run.
    pub(crate) fn set(&self, record: FinalRecord) {
        let _ = self.0.set(record);
    }

    /// The final record, once set — `None` before `plan::run_with_planner`
    /// reaches the point of building one (e.g. an aborted run, which returns
    /// `Err` before ever building it).
    pub(crate) fn get(&self) -> Option<&FinalRecord> {
        self.0.get()
    }
}
