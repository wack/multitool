//! The execution phase (M5; in-process rework in MULTI-1367; actor rework in
//! MULTI-1368).
//!
//! [`ExecutionActor`] receives one [`CheckDiscovered`] per validated check and
//! **immediately** offloads the (long-running) agent run onto a spawned task —
//! never `await`-ing it inline, since a Kameo actor processes one message at a
//! time to completion and awaiting here would serialize the pipeline. Each task
//! creates a CoW sandbox, runs the check's agent via the injected
//! [`CheckExecutor`] (which returns the verdict inline), drops the sandbox, and
//! either forwards a [`CheckCompleted`] to reporting or — if the agent did not
//! report — asks the actor to [`RetryCheck`] it.
//!
//! Each worker is **supervised** (not fire-and-forget): a second task joins its
//! handle and, if the worker panicked or was cancelled before delivering a
//! result, synthesizes a terminal *errored* [`CheckCompleted`]. Without this a
//! single panicked agent run would silently never report, leaving reporting's
//! expected-count one short forever and hanging the whole run.
//!
//! Bounded concurrency is enforced by a shared [`Semaphore`] acquired *inside*
//! each task (the semaphore is the cap, not the mailbox depth — decision #5).
//! Retries (`cfg.max_attempts`) are reframed as self-messages.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kameo::Actor;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use miette::Result;
use tokio::sync::Semaphore;

use crate::checks::executor::{AgentOutcome, CheckExecutor, CheckReport};
use crate::checks::messages::{
    CheckCompleted, CheckDiscovered, CheckJob, DiscoveryComplete, ExecutionComplete, RetryCheck,
};
use crate::checks::model::{Check, CheckId, CheckOutcome, Verdict};
use crate::checks::reporting::ReportingActor;
use crate::checks::sandbox::Sandbox;

/// The execution actor: turns a stream of discovered checks into a stream of
/// completed checks, fanning agent runs out onto bounded background tasks.
pub(crate) struct ExecutionActor {
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    working_dir: PathBuf,
    /// The concurrency cap, shared into every spawned task.
    semaphore: Arc<Semaphore>,
    /// How many times to (re)run a check whose agent fails to report (≥1).
    max_attempts: usize,
    /// The downstream reporting actor.
    reporting: ActorRef<ReportingActor>,
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
    pub(crate) fn new(
        executor: Arc<dyn CheckExecutor + Send + Sync>,
        sandbox: Arc<dyn Sandbox + Send + Sync>,
        working_dir: PathBuf,
        concurrency: usize,
        max_attempts: usize,
        reporting: ActorRef<ReportingActor>,
    ) -> Self {
        Self {
            executor,
            sandbox,
            working_dir,
            semaphore: Arc::new(Semaphore::new(concurrency.max(1))),
            max_attempts: max_attempts.max(1),
            reporting,
        }
    }

    /// Offload one attempt of `job` onto a background task. Returns immediately,
    /// keeping the actor mailbox responsive; the permit is acquired *inside* the
    /// task so the semaphore — not the mailbox — is the concurrency cap.
    fn dispatch(&self, ctx: &mut Context<Self, ()>, job: CheckJob, attempt: usize) {
        let executor = self.executor.clone();
        let sandbox = self.sandbox.clone();
        let semaphore = self.semaphore.clone();
        let working_dir = self.working_dir.clone();
        let reporting = self.reporting.clone();
        let me = ctx.actor_ref().clone();

        // Enough to synthesize a terminal verdict if the worker dies *without*
        // delivering one. Reporting finalizes only once it has folded exactly
        // `total_checks` outcomes, so a check that is counted but never reports
        // back leaves it one short forever — the run hangs and no actor shuts
        // down. A panic in `run_one` (sandbox clone, the agent, a poisoned judge
        // mutex, …) does exactly that: tokio swallows the panic of a detached
        // task, so the worker just vanishes. We therefore *supervise* the worker
        // below rather than fire-and-forget it.
        let guard_job = job.clone();
        let guard_reporting = reporting.clone();

        let worker = tokio::spawn(async move {
            // Acquire the permit inside the task. If the semaphore was closed the
            // pipeline is tearing down, so just drop the work.
            let Ok(_permit) = semaphore.acquire_owned().await else {
                return;
            };

            let result = run_one(executor, sandbox, job.id, job.check.clone(), &working_dir).await;

            if has_verdict(Some(&result)) {
                // The agent reported: reconcile and forward straight to reporting.
                let outcome = reconcile(Some(&result), &job.check.title);
                if let Err(err) = reporting.tell(CheckCompleted { job, outcome }).await {
                    tracing::debug!(?err, "reporting actor unavailable for completed check");
                }
            } else if let Err(err) = me
                .tell(RetryCheck {
                    job,
                    attempt,
                    last: result,
                })
                .await
            {
                tracing::debug!(?err, "execution actor unavailable for retry");
            }
        });

        // Supervise the worker: if it panicked or was cancelled before delivering
        // a `CheckCompleted`/`RetryCheck`, turn that into a terminal *errored*
        // verdict so the requirement still resolves and reporting's count can
        // never stall. A clean finish (`Ok`) already delivered its own signal, so
        // the guard stays silent — it cannot double-count.
        tokio::spawn(async move {
            if let Err(join_err) = worker.await {
                let outcome = CheckOutcome {
                    title: guard_job.check.title.clone(),
                    verdict: Verdict::Errored,
                    evidence: Some(format!("agent task terminated abnormally: {join_err}")),
                };
                if let Err(err) = guard_reporting
                    .tell(CheckCompleted {
                        job: guard_job,
                        outcome,
                    })
                    .await
                {
                    tracing::debug!(?err, "reporting actor unavailable for crashed check");
                }
            }
        });
    }

    /// Forward a terminal (attempts-exhausted, no-verdict) check to reporting as
    /// an errored outcome.
    async fn finish_errored(&self, job: CheckJob, last: Result<AgentOutcome>) {
        let outcome = reconcile(Some(&last), &job.check.title);
        if let Err(err) = self.reporting.tell(CheckCompleted { job, outcome }).await {
            tracing::debug!(?err, "reporting actor unavailable for errored check");
        }
    }
}

impl Message<CheckDiscovered> for ExecutionActor {
    type Reply = ();

    async fn handle(&mut self, msg: CheckDiscovered, ctx: &mut Context<Self, ()>) -> Self::Reply {
        self.dispatch(ctx, msg.job, 1);
    }
}

impl Message<RetryCheck> for ExecutionActor {
    type Reply = ();

    async fn handle(&mut self, msg: RetryCheck, ctx: &mut Context<Self, ()>) -> Self::Reply {
        let RetryCheck { job, attempt, last } = msg;
        if attempt < self.max_attempts {
            tracing::info!(
                check = job.id,
                attempt = attempt + 1,
                "retrying check whose agent did not report"
            );
            self.dispatch(ctx, job, attempt + 1);
        } else {
            self.finish_errored(job, last).await;
        }
    }
}

impl Message<DiscoveryComplete> for ExecutionActor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: DiscoveryComplete,
        _ctx: &mut Context<Self, ()>,
    ) -> Self::Reply {
        // Tell reporting how many completed checks to expect. Reporting gates
        // finalization on its own count, so this is race-free regardless of
        // whether agents have settled yet.
        if let Err(err) = self
            .reporting
            .tell(ExecutionComplete {
                total_checks: msg.total_checks,
            })
            .await
        {
            tracing::debug!(?err, "reporting actor unavailable for execution-complete");
        }
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
) -> Result<AgentOutcome> {
    let handle = sandbox.create(working_dir).await?;

    let request = crate::checks::executor::AgentRunRequest {
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
        )
        .await
        .unwrap();
        assert!(!out[0].satisfied);
        assert_eq!(out[0].check_outcomes[0].verdict, Verdict::Errored);
    }

    #[tokio::test]
    async fn panicking_check_errors_without_hanging_the_run() {
        // A check whose agent run panics must not wedge the pipeline: reporting
        // gates finalization on an exact count, and a panicked detached task is
        // swallowed by tokio, so without worker supervision `received` would stay
        // one short forever and the run would never produce a result. The other
        // requirement must still pass, and the crashed check must come back
        // errored. The timeout converts a regression (a hang) into a failure.
        let reqs = vec![
            req("Healthy", vec![("ok", "p-ok")]),
            req("Crashes", vec![("boom", "p-boom")]),
        ];
        let executor = Arc::new(
            FakeExecutor::new()
                .with_report(0, true, Some("fine"))
                .with_panic(1),
        );
        let cfg = configuration();
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            run_to_outcomes(
                &cfg,
                executor,
                Arc::new(NoopSandbox),
                &PathBuf::from("."),
                &reqs,
            ),
        )
        .await
        .expect("a crashed check must not hang the run")
        .unwrap();

        assert_eq!(out.len(), 2);
        assert!(out[0].satisfied, "the healthy requirement should pass");
        assert!(!out[1].satisfied, "the crashed requirement should not pass");
        assert_eq!(out[1].check_outcomes[0].verdict, Verdict::Errored);
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
        )
        .await
        .unwrap();
        assert!(out[0].satisfied);
        // Ran twice: one silent attempt, then one reporting attempt.
        assert_eq!(executor.seen().len(), 2);
    }
}
