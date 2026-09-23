//! The execution phase (M5; in-process rework in MULTI-1367; actor rework in
//! MULTI-1368; cersei-workflows fan-out in MULTI-1372).
//!
//! [`ExecutionActor`] **buffers** each [`CheckDiscovered`] job as discovery
//! streams it in, then — on the [`DiscoveryComplete`] sentinel — dispatches every
//! buffered check through a cersei **`foreach` workflow** whose bounded
//! concurrency is `cfg.concurrency`. The workflow replaces the hand-rolled
//! semaphore + per-check `tokio::spawn` + retry-self-message machinery: the engine
//! runs one `run-check` step per check, caps how many run at once, and each step
//! hands the injected [`CheckExecutor`] a lazy CoW sandbox lease alongside the
//! check's evidence source (MULTI-1818) — the executor decides whether it needs
//! the sandbox at all, acquires it (or not), returns the verdict inline, drops
//! the request (tearing any acquired sandbox down), harvests the attempt's
//! trace, and forwards a [`CheckCompleted`] to reporting.
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
use std::sync::atomic::{AtomicBool, Ordering};

use cersei_workflows::{FnStep, RunStatus, StepRegistry, Workflow, WorkflowBuilder};
use kameo::Actor;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use miette::Result;
use serde_json::{Value, json};

use crate::checks::executor::{
    AgentOutcome, CheckExecutor, CheckReport, PlanIdentity, ProgressSink,
};
#[cfg(feature = "jev")]
use crate::checks::jev::executor::AbortCheckRun;
#[cfg(feature = "jev")]
use crate::checks::messages::AbortRun;
use crate::checks::messages::{
    CheckCompleted, CheckDiscovered, CheckJob, DiscoveryComplete, ExecutionComplete,
};
use crate::checks::model::{CheckOutcome, DecidedBy, Verdict};
use crate::checks::presenter::{PresenterActor, UiEvent};
use crate::checks::reporting::ReportingActor;
use crate::checks::sandbox::{Sandbox, SandboxLease};
use crate::checks::trace_archive::{TraceCollector, TraceEntry};

/// The execution actor: buffers the discovered checks, then fans them out
/// through a bounded-concurrency cersei `foreach` workflow.
pub(crate) struct ExecutionActor {
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
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
    /// Whole-run abort flag (MULTI-1825): flips `true` the instant a check's
    /// executor fails with an unrecoverable Jev error
    /// (`Unauthorized`/`MissingApiKey`/a non-context `Invalid`, `--features
    /// jev` only — see `crate::checks::jev::executor`). Checked at the top of
    /// every `execute_check_job` call so a queued-but-not-yet-started check
    /// never runs (no agent, no report) once the run is aborting; a check
    /// already past that point runs to completion, but its outcome is never
    /// reported, since reporting's oneshot has already fired with the abort
    /// diagnostic by the time it would arrive. Always `false` in the default
    /// build — `CerseiExecutor` never returns an error that sets it — kept
    /// unconditional (rather than `#[cfg(feature = "jev")]`) so this field
    /// and the top-of-loop check need no per-build duplication; only the
    /// jev-specific pieces (the `AbortCheckRun` downcast and the `AbortRun`
    /// message that sets it) are feature-gated.
    aborted: Arc<AtomicBool>,
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
        concurrency: usize,
        max_attempts: usize,
        reporting: ActorRef<ReportingActor>,
        presenter: ActorRef<PresenterActor>,
        trace_collector: Option<Arc<TraceCollector>>,
    ) -> Self {
        Self {
            executor,
            sandbox,
            concurrency: concurrency.max(1),
            max_attempts: max_attempts.max(1),
            reporting,
            presenter,
            trace_collector,
            jobs: Vec::new(),
            aborted: Arc::new(AtomicBool::new(false)),
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
        let reporting = self.reporting.clone();
        let presenter = self.presenter.clone();
        let trace_collector = self.trace_collector.clone();
        let max_attempts = self.max_attempts;
        let aborted = self.aborted.clone();

        let registry = StepRegistry::new();
        registry.register(Arc::new(FnStep::new(
            "run-check",
            move |input: Value, _ctx| {
                let executor = executor.clone();
                let sandbox = sandbox.clone();
                let reporting = reporting.clone();
                let presenter = presenter.clone();
                let trace_collector = trace_collector.clone();
                let jobs = jobs.clone();
                let aborted = aborted.clone();
                async move {
                    let id = input.get("id").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let job = jobs[id].clone();
                    execute_check_job(
                        executor,
                        sandbox,
                        &presenter,
                        &reporting,
                        trace_collector.as_ref(),
                        max_attempts,
                        job,
                        &aborted,
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
    presenter: &ActorRef<PresenterActor>,
    reporting: &ActorRef<ReportingActor>,
    trace_collector: Option<&Arc<TraceCollector>>,
    max_attempts: usize,
    job: CheckJob,
    aborted: &Arc<AtomicBool>,
) {
    // Whole-run abort already signaled by another check (MULTI-1825,
    // `--features jev`; always `false` in the default build — see
    // `ExecutionActor::aborted`'s docs). A queued check that hasn't started
    // yet stops here: no presenter event, no agent run, no report.
    if aborted.load(Ordering::Relaxed) {
        return;
    }

    let id = job.id;
    let mut attempt = 1;
    loop {
        // The agent is about to run: mark the check Running. `attempt` lets
        // the presenter distinguish this attempt's progress from a
        // previous one's (MULTI-1828 code review) and clears any leftover
        // progress from before.
        let _ = presenter
            .tell(UiEvent::CheckStarted {
                id,
                attempt: attempt as u32,
            })
            .await;

        let mut result = run_one(executor.clone(), sandbox.clone(), presenter, &job, attempt).await;

        // An unrecoverable Jev failure (`--features jev` only —
        // `Unauthorized`/`MissingApiKey`/a non-context `Invalid`; see
        // `crate::checks::jev::executor`) aborts the whole run rather than
        // failing just this check: flip the shared flag so no further queued
        // check starts its agent, tell reporting the diagnostic directly
        // (bypassing the normal `CheckCompleted` path — reporting's oneshot
        // fires immediately, exactly like `DiscoveryFailed`), and return
        // without retrying. `CerseiExecutor` never produces this error, so
        // this branch is unreachable in the default build.
        #[cfg(feature = "jev")]
        if matches!(&result, Err(e) if e.downcast_ref::<AbortCheckRun>().is_some()) {
            aborted.store(true, Ordering::Relaxed);
            let Err(report) = result else {
                unreachable!("the `matches!` above just confirmed this is `Err`")
            };
            if let Err(err) = reporting.tell(AbortRun { report }).await {
                tracing::debug!(?err, "reporting actor unavailable for run abort");
            }
            return;
        }

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

/// Drive one check: hand the executor its evidence source plus a lazy CoW
/// sandbox lease over it, and let the executor decide whether it needs the
/// sandbox at all (MULTI-1818). The executor owns the agent lifecycle (the
/// in-process executor cancels its agent the instant it reports; the legacy
/// fallback runs the subprocess to completion or timeout) and, if it acquires
/// the lease, its teardown too — the lease lives inside the request, so
/// dropping the request when `run_check` returns tears the clone down (RAII).
///
/// The lease clones `job.root` — the requirement's repository root
/// (MULTI-1834), resolved per requirements file during discovery — not the
/// directory `multi check` was scanned from.
///
/// Also wires this attempt's [`ProgressSink`] (MULTI-1828) to `presenter`: a
/// forwarder task drains the paired receiver into fire-and-forget
/// `UiEvent::CheckProgress` tells while the executor runs. Progress must
/// never sit on the verdict path (MULTI-1828 code review), so this function
/// does **not** join the forwarder — an [`AbortOnDrop`] guard aborts it the
/// instant this function returns, on every exit path (success, executor error, or
/// this future itself being dropped/cancelled), rather than waiting for it
/// to drain. That is safe *only* because a stale/reordered progress update is
/// now the presenter state's problem, not an ordering guarantee this
/// function provides: [`UiEvent::CheckProgress`] carries `attempt`, and
/// `PresenterState` rejects one that doesn't match the row's current attempt
/// (see `presenter::state`).
async fn run_one(
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    presenter: &ActorRef<PresenterActor>,
    job: &CheckJob,
    attempt: usize,
) -> Result<AgentOutcome> {
    let declared_in = declared_in_relative_to_root(&job.filepath, &job.root);
    let attempt = u32::try_from(attempt).unwrap_or(u32::MAX);

    let (progress, mut updates) = ProgressSink::channel();
    let id = job.id;
    let forwarder = {
        let presenter = presenter.clone();
        tokio::spawn(async move {
            while let Some(update) = updates.recv().await {
                let _ = presenter
                    .tell(UiEvent::CheckProgress {
                        id,
                        attempt,
                        turn: update.turn,
                        max_turns: update.max_turns,
                        activity: update.activity,
                    })
                    .await;
            }
        })
    };
    // Aborts `forwarder` on drop — i.e. the instant this function returns by
    // any path — so it can never delay or block settlement, even if a
    // `ProgressSink` clone somehow outlived `run_check` (e.g. captured by a
    // leaked `Arc` on a timeout path) and would otherwise hold the channel
    // open forever.
    let _forwarder_guard = AbortOnDrop(forwarder);

    let request = crate::checks::executor::AgentRunRequest {
        check_id: job.id,
        check: job.check.clone(),
        source_dir: job.root.clone(),
        sandbox: SandboxLease::new(sandbox, job.root.clone()),
        declared_in,
        attempt,
        progress: Some(progress),
        plan: PlanIdentity {
            dir: crate::checks::model::plan_dir(&job.filepath),
            requirement_id: job.req_id.clone(),
            requirement_title: job.req_title.clone(),
            root_source: job.root_source,
        },
    };

    executor.run_check(request).await
}

/// Aborts the wrapped task when dropped. Used so the progress-forwarder task
/// (MULTI-1828) never outlives the attempt it belongs to: `run_one` never
/// awaits it, so this drop guard — running on every return path, including a
/// panic or this function's own future being cancelled — is the only thing
/// that stops it.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The declaring file's path relative to `root`, for display in the agent's
/// instructions (MULTI-1834), e.g. `services/keystore/CHECKS.toml`. Stated so
/// the agent retains the scoping a smaller, per-scan-directory sandbox used
/// to provide for free, now that the sandbox spans the whole repository root.
///
/// `filepath` may be relative to the process's current directory —
/// [`super::discovery::discover`] leaves discovered `CHECKS.toml` paths exactly
/// as found, so diagnostics elsewhere that name a file keep their
/// pre-MULTI-1834 display — while `root` is always absolute (resolved during
/// discovery). This absolutizes `filepath` before stripping `root` off, so
/// the result is correct regardless of the invocation directory or how deep
/// `job.root` sits above it.
///
/// `root` is always a lexical ancestor of the absolutized `filepath` by
/// construction (both are derived from the same scan root via the same
/// lexical `std::path::absolute` operation — see `discover` and
/// `repo_root::resolve`), so the `strip_prefix` below cannot fail in
/// practice. The fallback exists only to guarantee this *never* emits an
/// absolute host filesystem path into an agent's instructions if that
/// invariant is somehow violated (e.g. `std::path::absolute` itself errors,
/// which only happens if the process's current directory is unavailable): it
/// degrades to just the file's name, dropping directory context rather than
/// leaking the host path.
///
/// `pub(crate)` (not private): `crate::checks::plan` (MULTI-1824, under
/// `--features jev`) needs the exact same root-relative `declared_in` — both
/// for the agent's instructions (via `AgentRunRequest`, identically to `multi
/// check`) and for the Jev verification request's `state.requirement.declared_in`
/// (MULTI-1823) — and reuses this rather than duplicating it.
pub(crate) fn declared_in_relative_to_root(filepath: &Path, root: &Path) -> PathBuf {
    let absolute = std::path::absolute(filepath).unwrap_or_else(|_| filepath.to_path_buf());
    absolute
        .strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| {
            filepath
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| filepath.to_path_buf())
        })
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
            decided_by: decided_by(agent),
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
        // An errored check (no verdict at all — crashed, timed out, or the
        // executor itself errored) is never a `Cached`/`Jev` outcome (both
        // always carry a verdict — see `AgentOutcome::decided_by`'s docs), so
        // the default is always correct here, not just a placeholder.
        decided_by: DecidedBy::default(),
    }
}

fn reported_verdict(agent: Option<&Result<AgentOutcome>>) -> Option<CheckReport> {
    match agent {
        Some(Ok(o)) => o.verdict.clone(),
        _ => None,
    }
}

/// Which decision engine produced `agent`'s outcome (MULTI-1825) — see
/// [`AgentOutcome::decided_by`].
fn decided_by(agent: Option<&Result<AgentOutcome>>) -> DecidedBy {
    match agent {
        Some(Ok(o)) => o.decided_by,
        _ => DecidedBy::default(),
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
    // `figment::Jail`'s closure returns a large `Result`; unavoidable here.
    #![allow(clippy::result_large_err)]

    use crate::checks::config::configuration;
    use crate::checks::executor::FakeExecutor;
    use crate::checks::model::{Check, Requirement, RootSource, Verdict};
    use crate::checks::run_to_outcomes;
    use crate::checks::sandbox::NoopSandbox;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// Regression test for a code-review blocker on MULTI-1834: `job.filepath`
    /// is relative to the process's current directory (discovery leaves it
    /// exactly as found), and `job.root` is always absolute — `declared_in`
    /// must absolutize `filepath` before stripping `root`, or the two never
    /// share a common prefix.
    #[test]
    fn declared_in_strips_the_root_even_when_filepath_is_relative() {
        use figment::Jail;
        Jail::expect_with(|jail| {
            let root = jail.directory().to_path_buf();
            let filepath = PathBuf::from("services/keystore/CHECKS.toml");
            assert_eq!(
                super::declared_in_relative_to_root(&filepath, &root),
                PathBuf::from("services/keystore/CHECKS.toml")
            );
            Ok(())
        });
    }

    /// If `root` is somehow not an ancestor of `filepath` (unreachable in the
    /// real pipeline — see `declared_in_relative_to_root`'s doc comment), the
    /// result must degrade to just the file name, never leak the absolute
    /// host path into an agent's instructions.
    #[test]
    fn declared_in_never_leaks_an_absolute_path_on_mismatch() {
        let filepath = PathBuf::from("/some/unrelated/tree/CHECKS.toml");
        let root = PathBuf::from("/a/totally/different/root");
        let declared_in = super::declared_in_relative_to_root(&filepath, &root);
        assert_eq!(declared_in, PathBuf::from("CHECKS.toml"));
    }

    fn req(title: &str, checks: Vec<(&str, &str)>) -> Requirement {
        Requirement {
            filepath: PathBuf::from("CHECKS.toml"),
            id: crate::checks::discovery::slug(title),
            title: title.to_string(),
            description: None,
            tags: Vec::new(),
            checks: checks
                .into_iter()
                .enumerate()
                .map(|(i, (t, p))| Check::new_prompt(format!("c{i}"), t, p))
                .collect(),
            root: PathBuf::from("."),
            root_source: RootSource::ScanDirectory,
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

/// Pipeline-level abort tests for MULTI-1825's whole-run abort mechanism
/// (`--features jev` only): a real [`JevExecutor`](crate::checks::jev::executor::JevExecutor)
/// driven through the actual actor pipeline ([`crate::checks::run_to_outcomes`]),
/// not just `JevExecutor::run_check` in isolation — this is what proves the
/// `aborted` flag actually stops a second, queued check's agent from ever
/// starting, not merely that the first check's own call returns `Err`.
#[cfg(all(test, feature = "jev"))]
mod jev_abort_tests {
    use std::path::Path;
    use std::sync::Arc;

    use serde_json::json;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::checks::config::{JevConfig, configuration};
    use crate::checks::executor::{CheckExecutor, FakeExecutor, ReadOnlyTool};
    use crate::checks::jev::client::JevClient;
    use crate::checks::jev::error::TYPESAFE_API_KEY_VAR;
    use crate::checks::jev::executor::JevExecutor;
    use crate::checks::jev::plan_file::{
        Decider, JevCalibration, PlanCall, PlanCheck, PlanFile, PlanRequirement, PlanStore,
        Reading, prompt_xxh64,
    };
    use crate::checks::jev::replay::{self, CallChecksum};
    use crate::checks::model::{Check, Requirement, RootSource};
    use crate::checks::presenter::null_backend;
    use crate::checks::run_to_outcomes;
    use crate::checks::sandbox::NoopSandbox;

    static API_KEY_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn with_api_key<F, Fut, T>(body: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let _guard = API_KEY_ENV_LOCK.lock().await;
        let previous = std::env::var(TYPESAFE_API_KEY_VAR).ok();
        // SAFETY: serialized by `API_KEY_ENV_LOCK`.
        unsafe {
            std::env::set_var(TYPESAFE_API_KEY_VAR, "test-key");
        }
        let result = body().await;
        unsafe {
            match previous {
                Some(value) => std::env::set_var(TYPESAFE_API_KEY_VAR, value),
                None => std::env::remove_var(TYPESAFE_API_KEY_VAR),
            }
        }
        result
    }

    fn write_file(dir: &Path, relative: &str, content: &str) {
        let p = dir.join(relative);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    async fn read_checksum(root: &Path, relative: &str) -> String {
        let call =
            replay::replay_call(ReadOnlyTool::Read, &json!({"file_path": relative}), root).await;
        match call.checksums {
            Some(CallChecksum::Single(x)) => x,
            other => panic!("expected a single checksum, got {other:?}"),
        }
    }

    /// MULTI-1825 acceptance: an `Unauthorized` Jev failure aborts the whole
    /// run with the diagnostic and zero agent runs — including for a
    /// SECOND, still-queued check that has no plan entry at all (and so
    /// would ordinarily run the agent immediately). `cfg.concurrency = 1`
    /// makes dispatch order deterministic: check 0 (the one that aborts)
    /// is fully handled before check 1 is ever dispatched.
    #[tokio::test]
    async fn unauthorized_aborts_the_whole_run_and_stops_a_queued_check() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/a.rs", "fn a() {}\n");
        let xxh64 = read_checksum(dir.path(), "src/a.rs").await;

        let check0 = Check::new_prompt("check-a", "Check A", "prompt a");
        let entry = PlanCheck {
            id: check0.id.clone(),
            title: check0.title.clone(),
            prompt_xxh64: prompt_xxh64(&check0.title, check0.prompt()),
            decider: Decider::Jev,
            verdict: true,
            evidence: Some("cached".to_string()),
            jev: Some(JevCalibration {
                model: "jev-1.13.0".to_string(),
                noul: 0.9,
                control_noul: 0.02,
                reading: Some(Reading::Satisfied),
            }),
            calls: vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        };
        let plan = PlanFile::new(vec![PlanRequirement {
            id: "r".to_string(),
            title: "R".to_string(),
            checks: vec![entry],
        }]);
        PlanStore::write(dir.path(), &plan).unwrap();
        // Edited after freezing: replay reports `ReadsStale`, so check 0
        // consults Jev live — and that call 401s.
        write_file(dir.path(), "src/a.rs", "fn a() { /* changed */ }\n");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let jev_config = JevConfig {
            model: "jev-latest".to_string(),
            threshold: 0.75,
            base_url: server.uri(),
        };
        let jev_client = Arc::new(JevClient::new(server.uri()).unwrap());
        // Check 1 (id 1) would report satisfied if the agent ever ran it —
        // it must not.
        let inner = Arc::new(FakeExecutor::new().with_report(1, true, Some("must never run")));
        let exec: Arc<dyn CheckExecutor + Send + Sync> = Arc::new(JevExecutor::new(
            inner.clone(),
            jev_client,
            jev_config,
            false,
            false,
        ));

        // One requirement, two checks under the same `CHECKS.toml`: check 0
        // has the plan entry above; check 1 has no entry at all (would
        // ordinarily decide `Agent` immediately) — proving the abort stops
        // it before it ever starts.
        let check1 = Check::new_prompt("check-b", "Check B", "prompt b");
        let reqs = vec![Requirement {
            filepath: dir.path().join("CHECKS.toml"),
            id: "r".to_string(),
            title: "R".to_string(),
            description: None,
            tags: Vec::new(),
            checks: vec![check0, check1],
            root: dir.path().to_path_buf(),
            root_source: RootSource::Manifest,
        }];

        let mut cfg = configuration();
        cfg.concurrency = 1;

        let result = with_api_key(|| {
            run_to_outcomes(&cfg, exec, Arc::new(NoopSandbox), &reqs, null_backend())
        })
        .await;

        let err = result.expect_err("an Unauthorized Jev failure must abort the whole run");
        let rendered = format!("{err:?}");
        assert!(
            rendered.contains("rejected") || rendered.contains("unauthorized"),
            "the abort diagnostic should name the underlying Jev failure: {rendered}"
        );
        assert!(
            inner.seen().is_empty(),
            "zero agent runs across the whole run, including the queued check: {:?}",
            inner.seen()
        );
    }
}
