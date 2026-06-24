//! The in-memory domain model produced by discovery and consumed by execution.
//!
//! The two primary objects are the [`Requirement`] (a statement of fact about
//! the repository that must hold) and the [`Check`] (one of the steps used to
//! decide whether a requirement is satisfied). A requirement is satisfied iff
//! *all* of its checks pass (logical AND).
//!
//! Titles are **not** unique across the set — requirements and checks are
//! grouped by the file that declares them, never keyed on their titles.

use std::path::PathBuf;

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
        };
        let fail = CheckOutcome {
            title: "b".into(),
            verdict: Verdict::Failed,
            evidence: Some("nope".into()),
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
}
