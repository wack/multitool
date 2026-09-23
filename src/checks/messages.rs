//! The messages exchanged between the three pipeline actors (MULTI-1368).
//!
//! The pipeline is wired as a one-way, fire-and-forget chain — each edge is an
//! [`ActorRef::tell`](kameo::actor::ActorRef::tell), never `ask`:
//!
//! ```text
//! DiscoveryActor --CheckDiscovered--> ExecutionActor --CheckCompleted--> ReportingActor
//! ```
//!
//! With the old `Vec`-collection barriers gone, "done" is no longer implicit, so
//! each stage carries an explicit end-of-stream sentinel ([`DiscoveryComplete`] /
//! [`ExecutionComplete`]) so the next stage knows how many items to expect.

use std::path::PathBuf;

use crate::checks::model::{Check, CheckId, CheckOutcome, RootSource};

/// One validated check, ready to run, carried end-to-end through the pipeline so
/// reporting can group + order results without consulting the original suite.
#[derive(Clone)]
pub struct CheckJob {
    /// Run-unique id (also the cersei `session_id` suffix `multi-check-{id}`).
    pub id: CheckId,
    /// Which requirement (in declaration order) this check rolls up into.
    pub req_index: usize,
    /// The requirement's title, for the final per-requirement render.
    pub req_title: String,
    /// The `CHECKS.toml` that declared the requirement.
    pub filepath: PathBuf,
    /// The requirement's repository root (MULTI-1834): what execution
    /// sandboxes for this check — see [`crate::checks::model::Requirement::root`].
    pub root: PathBuf,
    /// How [`Self::root`] was determined (MULTI-1834) — carried through so
    /// the Jev decision engine (MULTI-1825) knows whether a plan is even
    /// usable for this check: a [`RootSource::ScanDirectory`] root isn't
    /// stable across invocations, so no plan is read or written for it.
    pub root_source: RootSource,
    /// The requirement's id (MULTI-1825's plan identity, with
    /// [`Check::id`]) — see [`crate::checks::model::Requirement::id`].
    pub req_id: String,
    /// The check itself (id, title, and kind).
    pub check: Check,
}

/// Coordinator -> Discovery: kick off the walk/parse/validate.
pub struct BeginDiscovery;

/// Discovery -> Execution: a single validated check is ready to execute.
pub struct CheckDiscovered {
    pub job: CheckJob,
}

/// Discovery -> Execution: discovery finished; exactly `total_checks` were
/// streamed (the end-of-stream sentinel for the discovery→execution edge).
pub struct DiscoveryComplete {
    pub total_checks: usize,
}

/// Execution -> Reporting: a single check reached a terminal verdict.
pub struct CheckCompleted {
    pub job: CheckJob,
    /// The reconciled verdict + evidence for this check.
    pub outcome: CheckOutcome,
}

/// Execution -> Reporting: how many checks Reporting should expect in total (the
/// end-of-stream sentinel for the execution→reporting edge). Reporting finalizes
/// once it has folded exactly this many [`CheckCompleted`]s.
pub struct ExecutionComplete {
    pub total_checks: usize,
}

/// Discovery -> Reporting: the suite contained an invalid `CHECKS.toml`; abort the
/// whole run (strict whole-run abort, decision #3). No checks were ever streamed,
/// so no agents are spawned.
pub struct DiscoveryFailed {
    pub report: miette::Report,
}

/// Execution -> Reporting (MULTI-1825, `--features jev` only): a check's
/// [`crate::checks::jev::executor::JevExecutor`] hit an unrecoverable Jev
/// failure (`Unauthorized`/`MissingApiKey`/a non-context `Invalid`) — abort
/// the whole run with the diagnostic, exactly like [`DiscoveryFailed`], but
/// raised from execution rather than discovery: a bad or missing TypeSafe
/// credential must not silently turn every remaining check into an agent
/// run. Mirrors `multi plan`'s `AbortPlanRun`'s "abort, not just-this-check"
/// treatment of the same error kinds.
#[cfg(feature = "jev")]
pub struct AbortRun {
    pub report: miette::Report,
}
