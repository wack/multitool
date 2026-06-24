//! The execution phase (M5).
//!
//! Run every check **in parallel** (bounded), each in its own CoW sandbox and
//! against its own MCP endpoint; then reconcile each check's verdict (the
//! MCP-reported `success` is authoritative) and aggregate checks into
//! per-requirement outcomes via logical AND.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use miette::{IntoDiagnostic, Result};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::checks::config::Config;
use crate::checks::executor::{
    AgentOutcome, AgentRunRequest, BoxedExecutor, CheckExecutor, assemble_instructions,
};
use crate::checks::mcp::{CheckReport, ResultServer, mcp_config_json};
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

/// Convenience wrapper used by the pipeline orchestrator: build the executor and
/// sandbox from configuration (DI) and run [`execute`].
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

    // Stand up the one MCP server with an endpoint per check.
    let ids: Vec<CheckId> = planned.iter().map(|p| p.id).collect();
    let mut server = ResultServer::start(&ids).await?;

    // Dispatch all checks concurrently, bounded by the configured limit.
    let permits = Arc::new(Semaphore::new(cfg.concurrency.max(1)));
    let mut set: JoinSet<(CheckId, Result<AgentOutcome>)> = JoinSet::new();
    for p in &planned {
        let permit = permits.clone().acquire_owned().await.into_diagnostic()?;
        let executor = executor.clone();
        let sandbox = sandbox.clone();
        let id = p.id;
        let instructions = assemble_instructions(&p.check);
        let endpoint_url = server.endpoint_url(id);
        let working_dir = working_dir.to_path_buf();
        set.spawn(async move {
            let _permit = permit;
            let outcome = run_one(
                executor,
                sandbox,
                id,
                instructions,
                &endpoint_url,
                &working_dir,
            )
            .await;
            (id, outcome)
        });
    }

    let mut agent_outcomes: HashMap<CheckId, Result<AgentOutcome>> = HashMap::new();
    while let Some(joined) = set.join_next().await {
        let (id, outcome) = joined.into_diagnostic()?;
        agent_outcomes.insert(id, outcome);
    }

    // Only wait on the MCP channel for checks that ran cleanly but did not
    // already deliver an inline verdict.
    let needed: HashSet<CheckId> = planned
        .iter()
        .filter(|p| matches!(agent_outcomes.get(&p.id), Some(Ok(o)) if o.reported.is_none()))
        .map(|p| p.id)
        .collect();
    let reports = server.collect(&needed, cfg.report_grace).await;
    server.shutdown().await;

    // Reconcile each check, then aggregate per requirement.
    let mut outcomes: HashMap<CheckId, CheckOutcome> = HashMap::new();
    for p in &planned {
        let outcome = reconcile(
            reports.get(&p.id).cloned(),
            agent_outcomes.get(&p.id),
            &p.check.title,
        );
        outcomes.insert(p.id, outcome);
    }
    Ok(aggregate_planned(requirements, &planned, outcomes))
}

/// Drive one check: sandbox → mcp-config → dispatch the agent.
async fn run_one(
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    id: CheckId,
    instructions: String,
    endpoint_url: &str,
    working_dir: &Path,
) -> Result<AgentOutcome> {
    let handle = sandbox.create(working_dir).await?;
    let config_file = write_mcp_config(&mcp_config_json(endpoint_url))?;

    let request = AgentRunRequest {
        check_id: id,
        instructions,
        working_dir: handle.path().to_path_buf(),
        mcp_config_path: config_file.path().to_path_buf(),
    };
    let outcome = executor.run_check(request).await;
    // Keep the sandbox and config file alive until the agent has finished.
    drop(config_file);
    drop(handle);
    outcome
}

/// Write the `--mcp-config` JSON to a temp file the agent can read.
fn write_mcp_config(json: &str) -> Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut file = tempfile::Builder::new()
        .prefix("multi-mcp-")
        .suffix(".json")
        .tempfile()
        .into_diagnostic()?;
    file.write_all(json.as_bytes()).into_diagnostic()?;
    file.flush().into_diagnostic()?;
    Ok(file)
}

/// Reconcile a single check's verdict. The MCP-reported result is authoritative;
/// an executor-inline report is the fallback; absence of both is an error.
fn reconcile(
    mcp_report: Option<CheckReport>,
    agent: Option<&Result<AgentOutcome>>,
    title: &str,
) -> CheckOutcome {
    if let Some(report) = mcp_report.or_else(|| inline_report(agent)) {
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
        Some(Ok(o)) if o.exited_cleanly => {
            "agent finished without calling report-check-result".to_string()
        }
        Some(Ok(o)) => format!(
            "agent exited without reporting (exit {:?}){}",
            o.exit_code,
            stderr_suffix(&o.stderr)
        ),
        Some(Err(e)) => format!("execution error: {e}"),
        None => "no result was collected for this check".to_string(),
    };
    CheckOutcome {
        title: title.to_string(),
        verdict: Verdict::Errored,
        evidence: Some(reason),
    }
}

fn inline_report(agent: Option<&Result<AgentOutcome>>) -> Option<CheckReport> {
    match agent {
        Some(Ok(o)) => o.reported.clone(),
        _ => None,
    }
}

fn stderr_suffix(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        let snippet: String = trimmed.chars().take(200).collect();
        format!(": {snippet}")
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
        cfg.report_grace = std::time::Duration::from_millis(50);
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
