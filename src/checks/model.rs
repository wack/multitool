//! The in-memory domain model produced by discovery and consumed by execution.
//!
//! The two primary objects are the [`Requirement`] (a statement of fact about
//! the repository that must hold) and the [`Check`] (one of the steps used to
//! decide whether a requirement is satisfied). A requirement is satisfied iff
//! *all* of its checks pass (logical AND).
//!
//! Titles are **not** unique across the set — requirements and checks are
//! grouped by the file that declares them, never keyed on their titles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A stable identifier for a check within a single `multi check` run.
///
/// Ids are assigned when the flat list of checks is built for execution and are
/// used to key the per-check MCP endpoint and to route reported results back to
/// the right check.
pub type CheckId = usize;

/// A requirement: a statement of fact about the repository that must be true.
///
/// Analogous to a *control* in compliance terminology. A requirement whose fact
/// is true is **satisfied**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The `CHECKS.md` file that declared this requirement.
    pub filepath: PathBuf,
    /// The requirement title (the text after the `# Requirement`/`# Req` sentinel).
    pub title: String,
    /// The checks that attest to this requirement.
    ///
    /// Invariant: guaranteed **non-empty** after discovery validation (M1). An
    /// empty `checks` is a discovery-time error, never surfaced to execution.
    pub checks: Vec<Check>,
    /// The repository this requirement is scoped to (MULTI-1834): the nearest
    /// ancestor of `filepath` that contains a MultiTool manifest, or the scan
    /// directory when no manifest exists above it. Every check under this
    /// requirement is sandboxed here — not at the directory `multi check` was
    /// invoked from — so the same requirement gets the same scope regardless
    /// of invocation, and a monorepo with one manifest per service scopes
    /// each service's requirements to that service.
    pub root: PathBuf,
    /// How [`Requirement::root`] was determined.
    pub root_source: RootSource,
}

/// Where a [`Requirement::root`] came from (MULTI-1834).
///
/// Later tickets (the frozen-plan machinery, MULTI-1820/1822/1823/1824/1825)
/// need this distinction to know whether a requirement's scope is anchored to
/// a real manifest or is only the scan-directory fallback, so it's modeled as
/// its own type rather than folded into a bare `bool`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSource {
    /// The nearest ancestor of the requirements file containing a MultiTool
    /// manifest (`MultiTool.toml` / `.json` / `.jsonc`).
    Manifest,
    /// No manifest exists above the requirements file; `root` falls back to
    /// the scan directory `multi check` was invoked with — exactly the
    /// pre-MULTI-1834 behavior, so manifest-less projects keep working.
    ScanDirectory,
}

/// A check: instructions for deciding whether a requirement is satisfied.
///
/// In the MVP every check is a `prompt`-type check whose body is a prompt for a
/// Claude Code agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// The check title. May be **inherited** from the requirement when the check
    /// is anonymous (the requirement declared no explicit `## Check`).
    pub title: String,
    /// The agent prompt: the raw Markdown body beneath the `## Check` (or the
    /// requirement prose, for an anonymous check).
    pub prompt: String,
}

/// The verdict for a single check after execution and reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The agent reported `success: true`.
    Satisfied,
    /// The agent reported `success: false`.
    Failed,
    /// The agent never reported (crash/timeout), or the executor itself errored.
    /// Non-satisfying, but distinguished from a clean `Failed` for reporting.
    Errored,
}

impl Verdict {
    /// Whether this verdict counts as satisfying the check. Only [`Verdict::Satisfied`]
    /// is satisfying; both [`Verdict::Failed`] and [`Verdict::Errored`] are not.
    pub fn is_satisfied(self) -> bool {
        matches!(self, Verdict::Satisfied)
    }
}

/// The outcome of a single check: its verdict plus any evidence the agent gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOutcome {
    /// The check's title (carried through from [`Check::title`]).
    pub title: String,
    /// The reconciled verdict.
    pub verdict: Verdict,
    /// Optional explanation: the agent's reported `evidence`, or a synthesized
    /// reason when the check errored.
    pub evidence: Option<String>,
    /// Which decision engine settled this check (MULTI-1825): the reasoning
    /// agent, a fresh cached verdict replayed from `.check-plan.toml`, or a
    /// Jev call. Carried straight through from
    /// [`crate::checks::executor::AgentOutcome::decided_by`] by
    /// [`crate::checks::execution::reconcile`]. Default [`DecidedBy::Agent`]
    /// (the only decider that exists in the default build). No default-build
    /// reader yet — rendering it is MULTI-1827.
    pub decided_by: DecidedBy,
}

/// Which decision engine settled a check (MULTI-1825). See
/// [`CheckOutcome::decided_by`] and
/// [`crate::checks::executor::AgentOutcome::decided_by`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecidedBy {
    /// The reasoning agent ran and reported a verdict — every check in the
    /// default build, and any `--features jev` check the frozen plan
    /// couldn't (or wouldn't) settle without one.
    #[default]
    Agent,
    /// The plan's frozen evidence replayed byte-identical (`Freshness::Fresh`):
    /// the stored verdict was reused with zero model calls. `--features jev`
    /// only.
    Cached,
    /// The plan's evidence had changed but Jev, consulted over the replayed
    /// evidence, still agreed with the stored verdict. `--features jev` only.
    Jev,
}

impl DecidedBy {
    /// The short tag shown beside a check whose decider wasn't the reasoning
    /// agent (MULTI-1827): `None` for [`DecidedBy::Agent`] — the only variant
    /// that occurs in the default build, so no default-build render path
    /// ever produces a tag — `Some("cached")` / `Some("jev")` otherwise.
    /// Shared by the live presenter (inline tree, heartbeat/TUI footer) and
    /// the final report's failing-check line so the wording never drifts
    /// between surfaces.
    pub(crate) fn tag(self) -> Option<&'static str> {
        match self {
            DecidedBy::Agent => None,
            DecidedBy::Cached => Some("cached"),
            DecidedBy::Jev => Some("jev"),
        }
    }
}

/// The `N cached · N jev · N agent` decider-count line (MULTI-1827): shown by
/// the inline TUI footer, the heartbeat summary, and the final report after
/// the requirement list. `None` when every settled check counted so far was
/// agent-decided (`cached == 0 && jev == 0`) — always true in the default
/// build, so this line never appears there, and true in a `jev` run until its
/// first cached/Jev-decided check settles.
pub(crate) fn decided_by_summary(cached: usize, jev: usize, agent: usize) -> Option<String> {
    if cached == 0 && jev == 0 {
        None
    } else {
        Some(format!("{cached} cached · {jev} jev · {agent} agent"))
    }
}

impl CheckOutcome {
    /// Whether this check was satisfied.
    pub fn is_satisfied(&self) -> bool {
        self.verdict.is_satisfied()
    }
}

/// The aggregate outcome of a requirement: satisfied iff *all* its checks are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirementOutcome {
    /// The requirement title.
    pub title: String,
    /// The file that declared the requirement.
    pub filepath: PathBuf,
    /// `true` iff every check in [`RequirementOutcome::check_outcomes`] is satisfied.
    pub satisfied: bool,
    /// The per-check outcomes, in declaration order.
    pub check_outcomes: Vec<CheckOutcome>,
}

// ---------------------------------------------------------------------------
// Plan identity (MULTI-1825)
// ---------------------------------------------------------------------------

/// The `(dir, source)` pair identifying which `.check-plan.toml` covers a
/// requirements file: `dir` is `filepath`'s parent directory and `source` is
/// its file name — the exact key
/// [`crate::checks::jev::plan_file::PlanRequirement::source`] stores and
/// [`crate::checks::jev::plan_file::PlanFile::lookup`] matches against.
/// `pub(crate)`, not `#[cfg(feature = "jev")]`: unconditional so `multi
/// check`'s `stream_requirements` (populating every `CheckJob`'s plan
/// identity, MULTI-1825) and `multi plan`'s `group_by_directory`
/// (MULTI-1824, `--features jev` only) derive it through the exact same
/// function — the two commands must agree byte-for-byte on where a given
/// requirement's plan lives.
pub(crate) fn plan_dir_and_source(filepath: &Path) -> (PathBuf, String) {
    let dir = filepath
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let source = filepath
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| filepath.display().to_string());
    (dir, source)
}

/// Derive each requirement's plan-identity `(dir, source, ordinal)` triple —
/// see [`plan_dir_and_source`]. `ordinal` is this requirement's 0-based
/// position among every requirement sharing the same `(dir, source)` pair,
/// i.e. its position within its own declaring file — the exact
/// `req_ordinal` [`crate::checks::jev::plan_file::PlanFile::lookup`] keys
/// on. Shared by `multi check`'s `stream_requirements` and `multi plan`'s
/// `group_by_directory` so both commands compute the identical ordinal for
/// the identical requirement.
pub(crate) fn requirement_plan_identities(
    requirements: &[Requirement],
) -> Vec<(PathBuf, String, u32)> {
    let mut ordinals: HashMap<(PathBuf, String), u32> = HashMap::new();
    requirements
        .iter()
        .map(|req| {
            let (dir, source) = plan_dir_and_source(&req.filepath);
            let counter = ordinals.entry((dir.clone(), source.clone())).or_insert(0);
            let ordinal = *counter;
            *counter += 1;
            (dir, source, ordinal)
        })
        .collect()
}

impl RequirementOutcome {
    /// Aggregate a requirement's check outcomes into a verdict via logical AND.
    pub fn aggregate(title: String, filepath: PathBuf, check_outcomes: Vec<CheckOutcome>) -> Self {
        let satisfied = check_outcomes.iter().all(CheckOutcome::is_satisfied);
        Self {
            title,
            filepath,
            satisfied,
            check_outcomes,
        }
    }

    /// The failing (non-satisfied) checks, for red reporting.
    pub fn failing_checks(&self) -> impl Iterator<Item = &CheckOutcome> {
        self.check_outcomes.iter().filter(|c| !c.is_satisfied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requirement_satisfied_only_when_all_checks_pass() {
        let pass = CheckOutcome {
            title: "a".into(),
            verdict: Verdict::Satisfied,
            evidence: None,
            decided_by: DecidedBy::Agent,
        };
        let fail = CheckOutcome {
            title: "b".into(),
            verdict: Verdict::Failed,
            evidence: Some("nope".into()),
            decided_by: DecidedBy::Agent,
        };

        let all_pass = RequirementOutcome::aggregate(
            "r".into(),
            "CHECKS.md".into(),
            vec![pass.clone(), pass.clone()],
        );
        assert!(all_pass.satisfied);

        let one_fail =
            RequirementOutcome::aggregate("r".into(), "CHECKS.md".into(), vec![pass, fail]);
        assert!(!one_fail.satisfied);
        assert_eq!(one_fail.failing_checks().count(), 1);
    }

    #[test]
    fn errored_check_does_not_satisfy() {
        assert!(!Verdict::Errored.is_satisfied());
        assert!(!Verdict::Failed.is_satisfied());
        assert!(Verdict::Satisfied.is_satisfied());
    }

    /// MULTI-1827 acceptance: `Agent` never gets a tag — the default build's
    /// only decider — while `Cached`/`Jev` get their exact ticket-specified
    /// words.
    #[test]
    fn only_non_agent_deciders_get_a_tag() {
        assert_eq!(DecidedBy::Agent.tag(), None);
        assert_eq!(DecidedBy::Cached.tag(), Some("cached"));
        assert_eq!(DecidedBy::Jev.tag(), Some("jev"));
    }

    /// MULTI-1827 acceptance: the decider-count summary is suppressed
    /// whenever every settled check was agent-decided (the default build,
    /// always), and rendered as `N cached · N jev · N agent` otherwise.
    #[test]
    fn decided_by_summary_is_gated_on_any_non_agent_decision() {
        assert_eq!(decided_by_summary(0, 0, 5), None);
        assert_eq!(
            decided_by_summary(2, 1, 3),
            Some("2 cached · 1 jev · 3 agent".to_string())
        );
        assert_eq!(
            decided_by_summary(0, 1, 0),
            Some("0 cached · 1 jev · 0 agent".to_string())
        );
    }
}
