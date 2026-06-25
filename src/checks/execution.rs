//! The execution phase (M5; in-process rework in MULTI-1367).
//!
//! Run every check **in parallel** (bounded), each in its own CoW sandbox, via
//! an in-process agent that reports its verdict through a per-check judge tool;
//! then reconcile each check's verdict (the reported `success` is authoritative)
//! and aggregate checks into per-requirement outcomes via logical AND.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use miette::{IntoDiagnostic, Result};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::checks::config::Config;
use crate::checks::executor::{
    AgentOutcome, AgentRunRequest, BoxedExecutor, CheckExecutor, CheckReport,
};
use crate::checks::model::{
    Check, CheckId, CheckOutcome, Requirement, RequirementOutcome, Verdict,
};
use crate::checks::sandbox::Sandbox;

/// A single check flattened out of the requirement set for execution, tagged
/// with its run-unique id and the requirement it rolls up into.
struct PlannedCheck {
    id: CheckId,
    req_index: usize,
    check: Check,
}

/// Convenience wrapper used by the pipeline orchestrator: build the sandbox from
/// configuration (DI) and run [`execute`] with the injected executor.
pub async fn execution_phase(
    cfg: &Config,
    executor: BoxedExecutor,
    working_dir: &Path,
    requirements: &[Requirement],
) -> Result<Vec<RequirementOutcome>> {
    let executor: Arc<dyn CheckExecutor + Send + Sync> = Arc::from(executor);
    let sandbox: Arc<dyn Sandbox + Send + Sync> =
        Arc::from(crate::checks::sandbox::select_sandbox());
    execute(cfg, executor, sandbox, working_dir, requirements).await
}

/// Run all checks and produce per-requirement outcomes.
///
/// `executor` and `sandbox` are injected so tests can substitute fakes.
pub async fn execute(
    cfg: &Config,
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    working_dir: &Path,
    requirements: &[Requirement],
) -> Result<Vec<RequirementOutcome>> {
    // Flatten checks, assigning each a run-unique id.
    let mut planned: Vec<PlannedCheck> = Vec::new();
    for (req_index, req) in requirements.iter().enumerate() {
        for check in &req.checks {
            planned.push(PlannedCheck {
                id: planned.len(),
                req_index,
                check: check.clone(),
            });
        }
    }

    // Nothing to run: every requirement trivially aggregates (empty AND = true,
    // though validation guarantees ≥1 check in practice).
    if planned.is_empty() {
        return Ok(aggregate(requirements, HashMap::new()));
    }

    // The most recent agent outcome per check. The reported verdict lives in
    // `AgentOutcome::verdict`.
    let mut last_outcome: HashMap<CheckId, Result<AgentOutcome>> = HashMap::new();

    // Re-run any check whose agent fails to report a verdict, up to
    // `max_attempts`. Agents are nondeterministic and occasionally hit the turn
    // cap, error, or time out without reporting; a fresh attempt usually
    // succeeds.
    let mut pending: Vec<&PlannedCheck> = planned.iter().collect();
    let attempts = cfg.max_attempts.max(1);
    for attempt in 1..=attempts {
        if pending.is_empty() {
            break;
        }
        if attempt > 1 {
            tracing::info!(
                attempt,
                checks = pending.len(),
                "retrying checks whose agent did not report"
            );
        }

        // Dispatch the pending checks concurrently, bounded by the limit.
        let permits = Arc::new(Semaphore::new(cfg.concurrency.max(1)));
        let mut set: JoinSet<(CheckId, Result<AgentOutcome>)> = JoinSet::new();
        for p in &pending {
            let permit = permits.clone().acquire_owned().await.into_diagnostic()?;
            let executor = executor.clone();
            let sandbox = sandbox.clone();
            let id = p.id;
            let check = p.check.clone();
            let working_dir = working_dir.to_path_buf();
            set.spawn(async move {
                let _permit = permit;
                let outcome = run_one(executor, sandbox, id, check, &working_dir).await;
                (id, outcome)
            });
        }
        while let Some(joined) = set.join_next().await {
            let (id, outcome) = joined.into_diagnostic()?;
            last_outcome.insert(id, outcome);
        }

        // Whatever still has no reported verdict is retried in the next round.
        pending = planned
            .iter()
            .filter(|p| !has_verdict(last_outcome.get(&p.id)))
            .collect();
    }

    // Reconcile each check, then aggregate per requirement.
    let mut outcomes: HashMap<CheckId, CheckOutcome> = HashMap::new();
    for p in &planned {
        let outcome = reconcile(last_outcome.get(&p.id), &p.check.title);
        outcomes.insert(p.id, outcome);
    }
    Ok(aggregate_planned(requirements, &planned, outcomes))
}

/// Whether an agent outcome carries a reported verdict.
fn has_verdict(outcome: Option<&Result<AgentOutcome>>) -> bool {
    matches!(outcome, Some(Ok(o)) if o.has_verdict())
}

/// Drive one check: create its CoW sandbox, run the agent against it, then tear
/// the sandbox down. The executor owns the agent lifecycle (the in-process
/// executor cancels its agent the instant it reports; the legacy fallback runs
/// the subprocess to completion or timeout).
async fn run_one(
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    id: CheckId,
    check: Check,
    working_dir: &Path,
) -> Result<AgentOutcome> {
    let handle = sandbox.create(working_dir).await?;

    let request = AgentRunRequest {
        check_id: id,
        check,
        working_dir: handle.path().to_path_buf(),
    };

    let outcome = executor.run_check(request).await;

    // Drop the sandbox after the run completes (RAII teardown of the clone).
    drop(handle);
    outcome
}

/// Reconcile a single check's verdict from its agent outcome. The reported
/// verdict ([`AgentOutcome::verdict`]) is authoritative; its absence is an error.
fn reconcile(agent: Option<&Result<AgentOutcome>>, title: &str) -> CheckOutcome {
    if let Some(report) = reported_verdict(agent) {
        let verdict = if report.success {
            Verdict::Satisfied
        } else {
            Verdict::Failed
        };
        return CheckOutcome {
            title: title.to_string(),
            verdict,
            evidence: report.evidence,
        };
    }

    let reason = match agent {
        Some(Ok(o)) => match &o.error {
            Some(err) => format!("agent errored without reporting: {err}"),
            None => format!(
                "agent finished without reporting a verdict{}{}",
                stop_reason_suffix(o.stop_reason.as_deref()),
                turns_suffix(o.turns),
            ),
        },
        Some(Err(e)) => format!("execution error: {e}"),
        None => "no result was collected for this check".to_string(),
    };
    CheckOutcome {
        title: title.to_string(),
        verdict: Verdict::Errored,
        evidence: Some(reason),
    }
}

fn reported_verdict(agent: Option<&Result<AgentOutcome>>) -> Option<CheckReport> {
    match agent {
        Some(Ok(o)) => o.verdict.clone(),
        _ => None,
    }
}

fn stop_reason_suffix(stop_reason: Option<&str>) -> String {
    match stop_reason {
        Some(r) if !r.is_empty() => format!(" (stop: {r})"),
        _ => String::new(),
    }
}

fn turns_suffix(turns: u32) -> String {
    if turns > 0 {
        format!(" after {turns} turns")
    } else {
        String::new()
    }
}

/// Aggregate when there are reconciled per-check outcomes.
fn aggregate_planned(
    requirements: &[Requirement],
    planned: &[PlannedCheck],
    mut outcomes: HashMap<CheckId, CheckOutcome>,
) -> Vec<RequirementOutcome> {
    let mut buckets: Vec<Vec<CheckOutcome>> = vec![Vec::new(); requirements.len()];
    for p in planned {
        if let Some(outcome) = outcomes.remove(&p.id) {
            buckets[p.req_index].push(outcome);
        }
    }
    requirements
        .iter()
        .zip(buckets)
        .map(|(req, checks)| {
            RequirementOutcome::aggregate(req.title.clone(), req.filepath.clone(), checks)
        })
        .collect()
}

/// Aggregate when there are no checks to run (degenerate suites).
fn aggregate(
    requirements: &[Requirement],
    _outcomes: HashMap<CheckId, CheckOutcome>,
) -> Vec<RequirementOutcome> {
    requirements
        .iter()
        .map(|req| {
            RequirementOutcome::aggregate(req.title.clone(), req.filepath.clone(), Vec::new())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::config::configuration;
    use crate::checks::executor::FakeExecutor;
    use crate::checks::sandbox::NoopSandbox;
    use std::path::PathBuf;

    fn req(title: &str, checks: Vec<(&str, &str)>) -> Requirement {
        Requirement {
            filepath: PathBuf::from("CHECKS.md"),
            title: title.to_string(),
            checks: checks
                .into_iter()
                .map(|(t, p)| Check {
                    title: t.to_string(),
                    prompt: p.to_string(),
                })
                .collect(),
        }
    }

    #[tokio::test]
    async fn requirement_satisfied_only_when_all_checks_pass() {
        // Two requirements: first has two passing checks (ids 0,1); second has
        // one failing check (id 2).
        let reqs = vec![
            req("Both pass", vec![("a", "pa"), ("b", "pb")]),
            req("One fails", vec![("c", "pc")]),
        ];
        let executor = Arc::new(
            FakeExecutor::new()
                .with_report(0, true, Some("a ok"))
                .with_report(1, true, None)
                .with_report(2, false, Some("c failed")),
        );
        let cfg = configuration();
        let out = execute(
            &cfg,
            executor.clone(),
            Arc::new(NoopSandbox),
            &PathBuf::from("."),
            &reqs,
        )
        .await
        .unwrap();

        assert_eq!(out.len(), 2);
        assert!(out[0].satisfied);
        assert!(!out[1].satisfied);
        assert_eq!(out[1].failing_checks().count(), 1);
        assert_eq!(
            out[1].check_outcomes[0].evidence.as_deref(),
            Some("c failed")
        );
        // All three checks were dispatched.
        assert_eq!(executor.seen().len(), 3);
    }

    #[tokio::test]
    async fn missing_report_errors_the_check_without_hanging() {
        let reqs = vec![req("Silent", vec![("c", "prompt")])];
        let executor = FakeExecutor::new().with_silent(0);
        let mut cfg = configuration();
        cfg.max_attempts = 1;
        let out = execute(
            &cfg,
            Arc::new(executor),
            Arc::new(NoopSandbox),
            &PathBuf::from("."),
            &reqs,
        )
        .await
        .unwrap();
        assert!(!out[0].satisfied);
        assert_eq!(out[0].check_outcomes[0].verdict, Verdict::Errored);
    }
}
