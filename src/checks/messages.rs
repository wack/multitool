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

use miette::Result;

use crate::checks::executor::AgentOutcome;
use crate::checks::model::{Check, CheckId, CheckOutcome};

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
    /// The `CHECKS.md` that declared the requirement.
    pub filepath: PathBuf,
    /// The check itself (title + prompt).
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

/// Execution self-message: a spawned agent run finished without reporting a
/// verdict; re-enqueue the same check (reframing the old retry loop as messages).
pub struct RetryCheck {
    pub job: CheckJob,
    /// The 1-based attempt number that just finished without a verdict.
    pub attempt: usize,
    /// The outcome of that attempt, kept so the terminal (attempts-exhausted)
    /// case can synthesize an accurate "errored" reason.
    pub last: Result<AgentOutcome>,
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

/// Discovery -> Reporting: the suite contained an invalid `CHECKS.md`; abort the
/// whole run (strict whole-run abort, decision #3). No checks were ever streamed,
/// so no agents are spawned.
pub struct DiscoveryFailed {
    pub report: miette::Report,
}
