//! `multi check`: validate declared non-functional ("ility") requirements by
//! running AI-agent checks.
//!
//! The feature is a **streaming actor pipeline** (MULTI-1368): three [Kameo]
//! actors wired one-way with fire-and-forget `tell`, so work flows through
//! incrementally rather than across `Vec`-collection barriers — a check begins
//! executing the moment discovery has validated it, and a result is folded into
//! reporting the moment its agent returns.
//!
//! ```text
//! DiscoveryActor --CheckDiscovered--> ExecutionActor --CheckCompleted--> ReportingActor
//! ```
//!
//! The phases, each in its own submodule:
//!
//! 1. [`config`] — the (hardcoded, dependency-injected) configuration phase.
//! 2. [`discovery`] — find/parse/validate `CHECKS.md` files, then stream each
//!    validated check downstream (strict whole-run abort on any invalid file).
//! 3. [`execution`] — run each check in a CoW [`sandbox`] via a boxed
//!    [`executor`], offloaded onto bounded background tasks; capture verdicts
//!    through each agent's in-process judge tool.
//! 4. [`reporting`] — fold verdicts incrementally, render, and produce the exit
//!    code.
//!
//! [Kameo]: https://docs.rs/kameo

pub mod config;
mod discovery;
mod execution;
pub mod executor;
mod messages;
pub mod model;
mod presenter;
mod reporting;
pub mod sandbox;
mod trace_archive;

#[cfg(test)]
mod e2e;

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use miette::{IntoDiagnostic, Result, miette};
use tokio::sync::oneshot;

use crate::checks::trace_archive::TraceCollector;

use crate::Terminal;
use crate::checks::config::{CliOverrides, Config};
use crate::checks::discovery::DiscoveryActor;
use crate::checks::execution::ExecutionActor;
use crate::checks::executor::CheckExecutor;
use crate::checks::messages::{BeginDiscovery, CheckDiscovered, CheckJob, DiscoveryComplete};
use crate::checks::model::{Requirement, RequirementOutcome};
use crate::checks::presenter::{PresenterActor, RenderBackend, UiEvent, select_backend};
use crate::checks::reporting::{ReportingActor, RunResult};
use crate::checks::sandbox::Sandbox;

/// Run the full `multi check` pipeline rooted at `working_dir`.
///
/// Returns the process exit code: `0` if every requirement is satisfied (an
/// empty tree counts as success), `1` if any requirement is unsatisfied.
/// Operational errors (e.g. an invalid `CHECKS.md`) surface as `Err` diagnostics
/// rather than an exit code, so CI can tell "checks failed" from "tool errored".
pub async fn run(terminal: &Terminal, working_dir: &Path, overrides: CliOverrides) -> Result<i32> {
    // Phase 1: configuration — resolve provider/model/effort
    // (flag > env > file) and construct the provider registry, injected forward.
    let resolved = config::load(overrides)?;

    tracing::debug!(
        provider = resolved.config.provider.as_str(),
        model = %resolved.config.model,
        concurrency = resolved.config.concurrency,
        available_providers = ?resolved.providers.keys().collect::<Vec<_>>(),
        "resolved checks configuration and provider registry",
    );

    // Build the injected executor (default: in-process cersei) and sandbox.
    let executor: Arc<dyn CheckExecutor + Send + Sync> = Arc::from(resolved.build_executor()?);
    let sandbox: Arc<dyn Sandbox + Send + Sync> = Arc::from(sandbox::select_sandbox());

    // Spawn the live presenter's backend up front (the inline viewport reserves
    // its terminal region immediately): inline TUI in a TTY, stderr heartbeat
    // otherwise. `owns_record` (true only for the inline TUI) routes the final
    // record below.
    let presenter::Backend {
        backend,
        owns_record,
    } = select_backend(terminal.stdout_allows_color());

    // If trace capture is enabled, create the shared collector now and hand a
    // clone to the pipeline: the executor fills each execution's trace and the
    // execution actor routes every attempt (retries included) here.
    let trace_collector = resolved
        .config
        .trace_archive
        .as_ref()
        .map(|_| Arc::new(TraceCollector::new()));

    // Phases 2–5: drive the actor pipeline (with the presenter) to its terminal
    // result, then render the record.
    let outcomes = run_pipeline(
        &resolved.config,
        executor,
        sandbox,
        working_dir,
        backend,
        trace_collector.clone(),
    )
    .await?;

    // Bundle the captured traces. Best-effort: a trace-archiving failure must not
    // fail an otherwise-successful check run. `run_pipeline` has already torn the
    // presenter down, so stderr is free for the notice.
    if let (Some(collector), Some(path)) =
        (&trace_collector, resolved.config.trace_archive.as_deref())
    {
        write_trace_archive(collector, path);
    }

    if owns_record {
        // The inline TUI was the sole terminal writer and has already flushed the
        // full record into scrollback; writing it again would double-print. Just
        // compute the exit code.
        Ok(reporting::exit_code(&outcomes))
    } else {
        // Heartbeat / no TTY: the reporting actor owns stdout — byte-for-byte as
        // MULTI-1368.
        reporting::report(terminal, &outcomes)
    }
}

/// Spawn the presenter + reporting + execution actors and return their refs plus
/// the channel the terminal result arrives on. Refs must be kept alive by the
/// caller until the result is received (dropping the last ref stops the actor).
///
/// The presenter is display-only: it isn't in the `tell` pipeline, but execution
/// holds its ref to fire-and-forget [`UiEvent`]s. The `backend` chooses where
/// those events surface (inline TUI / heartbeat / no-op).
fn spawn_core(
    cfg: &Config,
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    working_dir: &Path,
    backend: Box<dyn RenderBackend>,
    trace_collector: Option<Arc<TraceCollector>>,
) -> (
    ActorRef<ExecutionActor>,
    ActorRef<ReportingActor>,
    ActorRef<PresenterActor>,
    oneshot::Receiver<RunResult>,
) {
    let (tx, rx) = oneshot::channel();
    let presenter = PresenterActor::spawn(PresenterActor::new(backend, cfg.model.clone()));
    let reporting = ReportingActor::spawn(ReportingActor::new(tx));
    let execution = ExecutionActor::spawn(ExecutionActor::new(
        executor,
        sandbox,
        working_dir.to_path_buf(),
        cfg.concurrency,
        cfg.max_attempts,
        reporting.clone(),
        presenter.clone(),
        trace_collector,
    ));
    (execution, reporting, presenter, rx)
}

/// Stop `actor` and wait for its teardown to finish. `stop_gracefully` drains
/// any messages still in its mailbox first, so an actor's `on_stop` (the
/// presenter's terminal restore / final scrollback flush, in particular) sees a
/// complete state.
///
/// Every pipeline actor is stopped this way rather than merely having its
/// `ActorRef` dropped: an actor's mailbox only closes once *every* clone of its
/// `ActorRef` is gone, and [`ExecutionActor`]'s per-check background tasks each
/// hold clones of `execution`/`reporting`/`presenter` for their own lifetime
/// (see `execution::dispatch`). Relying on that implicit ref-counting to close
/// the mailbox — instead of sending an explicit `Signal::Stop`, which the actor
/// loop honors regardless of how many `ActorRef` clones are still outstanding —
/// makes teardown a race against those tasks actually finishing, rather than a
/// deterministic signal. An explicit stop is what let the presenter's shutdown
/// stay reliable; the other three actors need the same treatment.
async fn shutdown_actor<A: Actor>(actor: &ActorRef<A>) {
    let _ = actor.stop_gracefully().await;
    actor.wait_for_shutdown().await;
}

/// Stream a (validated) requirement set into the execution actor: one
/// [`CheckDiscovered`] per check — assigning each a run-unique [`CheckId`] from a
/// monotonic counter, in `(req_index, check)` declaration order — then a final
/// [`DiscoveryComplete`] sentinel.
///
/// Shared by [`DiscoveryActor`] (after its parse-all gate) and the test harness.
///
/// [`CheckId`]: crate::checks::model::CheckId
async fn stream_requirements(
    execution: &ActorRef<ExecutionActor>,
    presenter: &ActorRef<PresenterActor>,
    requirements: &[Requirement],
) -> Result<()> {
    let mut id = 0;
    let mut total = 0;
    for (req_index, req) in requirements.iter().enumerate() {
        for check in &req.checks {
            // Tell the presenter about the check first so its tree row exists
            // before execution can emit `CheckStarted` for it.
            let _ = presenter
                .tell(UiEvent::CheckQueued {
                    id,
                    req_index,
                    req_title: req.title.clone(),
                    check_title: check.title.clone(),
                })
                .await;
            let job = CheckJob {
                id,
                req_index,
                req_title: req.title.clone(),
                filepath: req.filepath.clone(),
                check: check.clone(),
            };
            execution
                .tell(CheckDiscovered { job })
                .await
                .map_err(|e| miette!("failed to enqueue discovered check: {e}"))?;
            id += 1;
            total += 1;
        }
    }
    let _ = presenter
        .tell(UiEvent::DiscoveryComplete {
            total_checks: total,
        })
        .await;
    execution
        .tell(DiscoveryComplete {
            total_checks: total,
        })
        .await
        .map_err(|e| miette!("failed to signal discovery completion: {e}"))?;
    Ok(())
}

/// Build and write the opt-in session-trace archive. Best-effort: on failure it
/// logs and returns rather than failing an otherwise-successful check run. Only
/// called when `--trace-archive` is set.
fn write_trace_archive(collector: &TraceCollector, path: &Path) {
    if collector.is_empty() {
        tracing::info!("trace capture enabled but no check executions ran; no archive written");
        return;
    }
    match trace_archive::write_archive(collector, path) {
        Ok(count) => {
            tracing::info!(count, path = %path.display(), "wrote check session-trace archive");
            let _ = writeln!(
                std::io::stderr(),
                "Wrote {count} check session trace(s) to {}",
                path.display()
            );
        }
        Err(e) => tracing::error!(
            error = %e,
            path = %path.display(),
            "failed to write check session-trace archive",
        ),
    }
}

/// Await the pipeline's terminal result, mapping a dropped channel (a dead
/// reporting actor) to a diagnostic so a crashed actor fails the run rather than
/// hanging it.
async fn await_result(rx: oneshot::Receiver<RunResult>) -> Result<Vec<RequirementOutcome>> {
    match rx.await {
        Ok(result) => result,
        Err(_) => Err(miette!(
            "the reporting actor terminated before producing a result"
        )),
    }
}

/// Drive the full pipeline (all three actors) over `working_dir` and return the
/// ordered per-requirement outcomes (or the abort diagnostic from an invalid
/// suite). The actor refs are held alive until the terminal result arrives.
async fn run_pipeline(
    cfg: &Config,
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    working_dir: &Path,
    backend: Box<dyn RenderBackend>,
    trace_collector: Option<Arc<TraceCollector>>,
) -> Result<Vec<RequirementOutcome>> {
    let (execution, reporting, presenter, rx) = spawn_core(
        cfg,
        executor,
        sandbox,
        working_dir,
        backend,
        trace_collector,
    );
    let discovery = DiscoveryActor::spawn(DiscoveryActor::new(
        working_dir.to_path_buf(),
        execution.clone(),
        reporting.clone(),
        presenter.clone(),
    ));
    discovery
        .tell(BeginDiscovery)
        .await
        .into_diagnostic()
        .map_err(|e| miette!("failed to start discovery: {e}"))?;

    let outcomes = await_result(rx).await;
    // The run is over: stop every actor explicitly rather than merely dropping
    // its `ActorRef` (see `shutdown_actor`). Presenter goes last so it's
    // guaranteed to have drained every `UiEvent` the other actors' own
    // shutdown might still emit (e.g. a final log line) before its terminal
    // restore / scrollback flush runs.
    shutdown_actor(&discovery).await;
    shutdown_actor(&execution).await;
    shutdown_actor(&reporting).await;
    shutdown_actor(&presenter).await;
    outcomes
}

/// Test-only entrypoint: drive **execution + reporting** over an in-memory,
/// already-validated requirement set (bypassing discovery), returning the
/// ordered outcomes. Lets execution-phase tests inject fakes without writing
/// `CHECKS.md` files.
#[cfg(test)]
async fn run_to_outcomes(
    cfg: &Config,
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    working_dir: &Path,
    requirements: &[Requirement],
    backend: Box<dyn RenderBackend>,
) -> Result<Vec<RequirementOutcome>> {
    let (execution, reporting, presenter, rx) =
        spawn_core(cfg, executor, sandbox, working_dir, backend, None);
    stream_requirements(&execution, &presenter, requirements).await?;
    let outcomes = await_result(rx).await;
    shutdown_actor(&execution).await;
    shutdown_actor(&reporting).await;
    shutdown_actor(&presenter).await;
    outcomes
}
