//! The execution phase (M5; in-process rework in MULTI-1367; actor rework in
//! MULTI-1368; cersei-workflows fan-out in MULTI-1372).
//!
//! [`ExecutionActor`] **buffers** each [`CheckDiscovered`] job as discovery
//! streams it in, then — on the [`DiscoveryComplete`] sentinel — dispatches every
//! buffered check through a cersei **`foreach` workflow** whose bounded
//! concurrency is `cfg.concurrency`. The workflow replaces the hand-rolled
//! semaphore + per-check `tokio::spawn` + retry-self-message machinery: the engine
//! runs one `run-check` step per check, caps how many run at once, and each step
//! creates a CoW sandbox, runs the check's agent via the injected
//! [`CheckExecutor`] (which returns the verdict inline), drops the sandbox,
//! harvests the attempt's trace, and forwards a [`CheckCompleted`] to reporting.
//!
//! A check whose agent never reports a verdict is retried **in place** (up to
//! `cfg.max_attempts`) inside the step, escalating the attempt number so the
//! executor can vary temperature/instructions — the same retry policy the old
//! `RetryCheck` self-message implemented, now a plain loop. Buffering until the
//! sentinel is behaviorally equivalent to the old burst dispatch: discovery
//! parse-gates the whole suite before streaming a single check, so all jobs are
//! always known by the time the workflow starts.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cersei_workflows::{FnStep, RunStatus, StepRegistry, Workflow, WorkflowBuilder};
use kameo::Actor;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use miette::Result;
use serde_json::{Value, json};

use crate::checks::executor::{AgentOutcome, CheckExecutor, CheckReport};
use crate::checks::messages::{
    CheckCompleted, CheckDiscovered, CheckJob, DiscoveryComplete, ExecutionComplete,
};
use crate::checks::model::{Check, CheckId, CheckOutcome, Verdict};
use crate::checks::presenter::{PresenterActor, UiEvent};
use crate::checks::reporting::ReportingActor;
use crate::checks::sandbox::Sandbox;
use crate::checks::trace_archive::{TraceCollector, TraceEntry};

/// The execution actor: buffers the discovered checks, then fans them out
/// through a bounded-concurrency cersei `foreach` workflow.
pub(crate) struct ExecutionActor {
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    working_dir: PathBuf,
    /// The bounded-concurrency cap handed to the `foreach` workflow (≥1).
    concurrency: usize,
    /// How many times to (re)run a check whose agent fails to report (≥1).
    max_attempts: usize,
    /// The downstream reporting actor.
    reporting: ActorRef<ReportingActor>,
    /// The display-only presenter, told of each check's lifecycle milestones.
    presenter: ActorRef<PresenterActor>,
    /// Opt-in sink for per-execution session traces (`multi check
    /// --trace-archive`). `None` disables capture. Shared across the concurrent
    /// `run-check` steps; every attempt (including retries) pushes its trace here
    /// before signalling completion downstream.
    trace_collector: Option<Arc<TraceCollector>>,
    /// Checks buffered as discovery streams them, drained into the workflow when
    /// the [`DiscoveryComplete`] sentinel arrives.
    jobs: Vec<CheckJob>,
}

impl Actor for ExecutionActor {
    type Args = Self;
    type Error = std::convert::Infallible;

    async fn on_start(
        args: Self::Args,
        _actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        Ok(args)
    }
}

impl ExecutionActor {
    /// Build the actor. `concurrency` and `max_attempts` are clamped to ≥1.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        executor: Arc<dyn CheckExecutor + Send + Sync>,
        sandbox: Arc<dyn Sandbox + Send + Sync>,
        working_dir: PathBuf,
        concurrency: usize,
        max_attempts: usize,
        reporting: ActorRef<ReportingActor>,
        presenter: ActorRef<PresenterActor>,
        trace_collector: Option<Arc<TraceCollector>>,
    ) -> Self {
        Self {
            executor,
            sandbox,
            working_dir,
            concurrency: concurrency.max(1),
            max_attempts: max_attempts.max(1),
            reporting,
            presenter,
            trace_collector,
            jobs: Vec::new(),
        }
    }

    /// Run every buffered check through a single-node `foreach` workflow whose
    /// bounded concurrency is `self.concurrency`. Each array element is a `{ id }`
    /// pointer into `jobs`; the `run-check` step looks the job up, drives its
    /// (possibly retried) agent run, and `tell`s reporting directly — so no domain
    /// type ever has to cross the workflow's JSON boundary. Awaited inline: this
    /// is the actor's last message, so blocking its handler until every check
    /// settles is exactly the intended barrier and needs no detached tasks.
    async fn run_checks(&self, jobs: Vec<CheckJob>) {
        let count = jobs.len();
        let jobs = Arc::new(jobs);

        let executor = self.executor.clone();
        let sandbox = self.sandbox.clone();
        let working_dir = self.working_dir.clone();
        let reporting = self.reporting.clone();
        let presenter = self.presenter.clone();
        let trace_collector = self.trace_collector.clone();
        let max_attempts = self.max_attempts;

        let registry = StepRegistry::new();
        registry.register(Arc::new(FnStep::new(
            "run-check",
            move |input: Value, _ctx| {
                let executor = executor.clone();
                let sandbox = sandbox.clone();
                let working_dir = working_dir.clone();
                let reporting = reporting.clone();
                let presenter = presenter.clone();
                let trace_collector = trace_collector.clone();
                let jobs = jobs.clone();
                async move {
                    let id = input.get("id").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let job = jobs[id].clone();
                    execute_check_job(
                        executor,
                        sandbox,
                        &working_dir,
                        &presenter,
                        &reporting,
                        trace_collector.as_ref(),
                        max_attempts,
                        job,
                    )
                    .await;
                    Ok(json!({ "id": id }))
                }
            },
        )));

        let def = WorkflowBuilder::new("checks")
            .foreach("run-check", self.concurrency)
            .commit();
        let wf = match Workflow::compile(def, &registry) {
            Ok(wf) => wf,
            Err(err) => {
                tracing::error!(?err, "failed to compile the checks workflow");
                return;
            }
        };

        let items: Vec<Value> = (0..count).map(|id| json!({ "id": id })).collect();
        match wf.start(Value::Array(items)).await {
            Ok(result) if result.status == RunStatus::Success => {}
            Ok(result) => tracing::warn!(
                status = ?result.status,
                error = ?result.error,
                "checks workflow did not run to success"
            ),
            Err(err) => tracing::error!(?err, "checks workflow failed to run"),
        }
    }
}

impl Message<CheckDiscovered> for ExecutionActor {
    type Reply = ();

    async fn handle(&mut self, msg: CheckDiscovered, _ctx: &mut Context<Self, ()>) -> Self::Reply {
        // Buffer; the whole set is dispatched at once on `DiscoveryComplete`.
        self.jobs.push(msg.job);
    }
}

impl Message<DiscoveryComplete> for ExecutionActor {
    type Reply = ();

    async fn handle(
        &mut self,
        _msg: DiscoveryComplete,
        _ctx: &mut Context<Self, ()>,
    ) -> Self::Reply {
        // Drain the buffer and fan every check out through the workflow (awaited
        // inline). Its `run-check` steps `tell` reporting one `CheckCompleted`
        // each as they settle.
        let jobs = std::mem::take(&mut self.jobs);
        let total = jobs.len();
        if !jobs.is_empty() {
            self.run_checks(jobs).await;
        }

        // Tell reporting how many completed checks to expect. Reporting gates
        // finalization on its own fold count, so sending this after the workflow
        // (every `CheckCompleted` already delivered) still finalizes correctly.
        if let Err(err) = self
            .reporting
            .tell(ExecutionComplete {
                total_checks: total,
            })
            .await
        {
            tracing::debug!(?err, "reporting actor unavailable for execution-complete");
        }
    }
}

/// Drive one check to a terminal outcome: run its agent, retrying in place (up to
/// `max_attempts`) while the agent fails to report, then `tell` reporting the
/// reconciled [`CheckCompleted`]. Fires the presenter's lifecycle events in the
/// same order the old spawn+self-message path did:
/// `CheckStarted, [CheckRetrying, CheckStarted]*, CheckSettled`.
#[allow(clippy::too_many_arguments)]
async fn execute_check_job(
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    working_dir: &Path,
    presenter: &ActorRef<PresenterActor>,
    reporting: &ActorRef<ReportingActor>,
    trace_collector: Option<&Arc<TraceCollector>>,
    max_attempts: usize,
    job: CheckJob,
) {
    let id = job.id;
    let mut attempt = 1;
    loop {
        // The agent is about to run: mark the check Running.
        let _ = presenter.tell(UiEvent::CheckStarted { id }).await;

        let mut result = run_one(
            executor.clone(),
            sandbox.clone(),
            job.id,
            job.check.clone(),
            working_dir,
            attempt,
        )
        .await;

        // Harvest this attempt's trace *before* signalling completion, so it is
        // collected even for retried attempts (whose outcome never reaches
        // reporting) and is race-free with run finalization.
        if let Some(collector) = trace_collector
            && let Some(bytes) = result.as_mut().ok().and_then(|o| o.trace_jsonl.take())
        {
            collector.push(TraceEntry {
                req_index: job.req_index,
                req_title: job.req_title.clone(),
                check_id: job.id,
                check_title: job.check.title.clone(),
                attempt,
                bytes,
            });
        }

        // Reported a verdict, or attempts exhausted: reconcile, surface to the
        // presenter, and forward the terminal outcome to reporting.
        if has_verdict(Some(&result)) || attempt >= max_attempts {
            let outcome = reconcile(Some(&result), &job.check.title);
            let _ = presenter
                .tell(UiEvent::CheckSettled {
                    id,
                    outcome: outcome.clone(),
                })
                .await;
            if let Err(err) = reporting.tell(CheckCompleted { job, outcome }).await {
                tracing::debug!(?err, "reporting actor unavailable for settled check");
            }
            return;
        }

        // No verdict and attempts remain: retry with an escalated attempt number
        // (so the executor can vary temperature/instructions).
        tracing::info!(
            check = id,
            attempt = attempt + 1,
            "retrying check whose agent did not report"
        );
        let _ = presenter
            .tell(UiEvent::CheckRetrying {
                id,
                attempt: attempt as u32,
            })
            .await;
        attempt += 1;
    }
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
    attempt: usize,
) -> Result<AgentOutcome> {
    let handle = sandbox.create(working_dir).await?;

    let request = crate::checks::executor::AgentRunRequest {
        check_id: id,
        check,
        working_dir: handle.path().to_path_buf(),
        attempt: u32::try_from(attempt).unwrap_or(u32::MAX),
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

#[cfg(test)]
mod tests {
    use crate::checks::config::configuration;
    use crate::checks::executor::FakeExecutor;
    use crate::checks::model::{Check, Requirement, Verdict};
    use crate::checks::run_to_outcomes;
    use crate::checks::sandbox::NoopSandbox;
    use std::path::PathBuf;
    use std::sync::Arc;

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
        let out = run_to_outcomes(
            &cfg,
            executor.clone(),
            Arc::new(NoopSandbox),
            &PathBuf::from("."),
            &reqs,
            crate::checks::presenter::null_backend(),
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
        let out = run_to_outcomes(
            &cfg,
            Arc::new(executor),
            Arc::new(NoopSandbox),
            &PathBuf::from("."),
            &reqs,
            crate::checks::presenter::null_backend(),
        )
        .await
        .unwrap();
        assert!(!out[0].satisfied);
        assert_eq!(out[0].check_outcomes[0].verdict, Verdict::Errored);
    }

    #[tokio::test]
    async fn check_is_retried_until_it_reports() {
        // A check that is silent on its first attempt but reports on a later one
        // must end up satisfied — exercising the retry self-message path.
        let reqs = vec![req("Eventually", vec![("c", "prompt")])];
        let executor = Arc::new(FakeExecutor::new().with_silent_until(0, 2, true, Some("ok now")));
        let cfg = configuration(); // max_attempts = 3
        let out = run_to_outcomes(
            &cfg,
            executor.clone(),
            Arc::new(NoopSandbox),
            &PathBuf::from("."),
            &reqs,
            crate::checks::presenter::null_backend(),
        )
        .await
        .unwrap();
        assert!(out[0].satisfied);
        // Ran twice: one silent attempt, then one reporting attempt — and the
        // executor was told which attempt each was (retries must be able to
        // vary temperature/instructions rather than replaying attempt 1).
        assert_eq!(executor.seen_attempts(), vec![(0, 1), (0, 2)]);
    }
}
