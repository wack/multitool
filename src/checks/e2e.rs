//! End-to-end pipeline test (MULTI-1355; actor pipeline in MULTI-1368).
//!
//! Drives the real discovery → execution → reporting → exit-code actor pipeline
//! over a fixture tree, with the fake executor and a no-op sandbox injected so
//! verdicts are scripted. Runs deterministically in CI with no `claude`,
//! network, or APFS dependency, guarding the end-to-end contract against
//! regressions.

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use clap::Parser;
use miette::Result;
use tempfile::TempDir;
use tokio::sync::Barrier;

use crate::checks::config::configuration;
use crate::checks::discovery::discover;
use crate::checks::executor::{
    AgentOutcome, AgentRunRequest, CheckExecutor, CheckReport, FakeExecutor,
};
use crate::checks::presenter::null_backend;
use crate::checks::reporting::report;
use crate::checks::sandbox::NoopSandbox;
use crate::checks::{run_pipeline, run_to_outcomes};
use crate::{Cli, Terminal};

/// A terminal with color forced off, so reporting emits deterministic text.
fn plain_terminal() -> Terminal {
    let cli = Cli::parse_from(["multi", "--enable-colors", "never"]);
    Terminal::new(&cli)
}

/// A fixture tree spanning two files: a satisfied anonymous-check requirement, a
/// two-check (AND) requirement, and — nested — a failing requirement.
fn write_fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("CHECKS.md"),
        "# Requirement Anon Satisfied\n\
         scan and confirm this holds\n\
         \n\
         # Requirement Multi And\n\
         ## Check First\n\
         first prompt\n\
         ## Check Second\n\
         second prompt\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("nested")).unwrap();
    fs::write(
        dir.path().join("nested/CHECKS.md"),
        "# Requirement Failing\n\
         ## Check Bad\n\
         this one fails\n",
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn pipeline_satisfied_failed_multi_and_anonymous() {
    let dir = write_fixture();
    let reqs = discover(dir.path()).await.unwrap();

    // Discovery: three requirements across two files, sorted (root before nested).
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[0].title, "Anon Satisfied");
    assert_eq!(reqs[1].title, "Multi And");
    assert_eq!(reqs[2].title, "Failing");
    // The anonymous check inherits the requirement's title.
    assert_eq!(reqs[0].checks.len(), 1);
    assert_eq!(reqs[0].checks[0].title, "Anon Satisfied");
    // The multi-check requirement carries two checks (ANDed).
    assert_eq!(reqs[1].checks.len(), 2);

    // Script the fake by check id (ids are assigned in requirement/check order).
    // Everything passes except the "Failing" requirement's check.
    let mut fake = FakeExecutor::new();
    let mut id = 0;
    let mut total = 0;
    for req in &reqs {
        for _ in &req.checks {
            let pass = req.title != "Failing";
            fake = fake.with_report(id, pass, Some(if pass { "ok" } else { "nope" }));
            id += 1;
            total += 1;
        }
    }
    let fake = Arc::new(fake);

    let cfg = configuration();
    let outcomes = run_to_outcomes(
        &cfg,
        fake.clone(),
        Arc::new(NoopSandbox),
        dir.path(),
        &reqs,
        null_backend(),
    )
    .await
    .unwrap();

    // Aggregated verdicts: satisfied / satisfied(AND) / failed.
    assert!(
        outcomes[0].satisfied,
        "anonymous-check requirement should pass"
    );
    assert!(
        outcomes[1].satisfied,
        "multi-check AND requirement should pass"
    );
    assert!(!outcomes[2].satisfied, "failing requirement should fail");
    assert_eq!(outcomes[2].failing_checks().count(), 1);
    assert_eq!(
        outcomes[2].check_outcomes[0].evidence.as_deref(),
        Some("nope")
    );

    // Every check was dispatched exactly once.
    assert_eq!(fake.seen().len(), total);

    // Reporting renders without error and yields exit 1 (one requirement failed).
    let code = report(&plain_terminal(), &outcomes).unwrap();
    assert_eq!(code, 1);
}

#[tokio::test]
async fn all_satisfied_exits_zero() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("CHECKS.md"), "# Requirement Ok\ndo it\n").unwrap();
    let reqs = discover(dir.path()).await.unwrap();

    let fake = Arc::new(FakeExecutor::new().with_report(0, true, None));
    let cfg = configuration();
    let outcomes = run_to_outcomes(
        &cfg,
        fake,
        Arc::new(NoopSandbox),
        dir.path(),
        &reqs,
        null_backend(),
    )
    .await
    .unwrap();
    assert!(outcomes[0].satisfied);

    let code = report(&plain_terminal(), &outcomes).unwrap();
    assert_eq!(code, 0);
}

#[tokio::test]
async fn empty_tree_exits_zero() {
    let dir = TempDir::new().unwrap();
    let reqs = discover(dir.path()).await.unwrap();
    assert!(reqs.is_empty());

    let cfg = configuration();
    let outcomes = run_to_outcomes(
        &cfg,
        Arc::new(FakeExecutor::new()),
        Arc::new(NoopSandbox),
        dir.path(),
        &reqs,
        null_backend(),
    )
    .await
    .unwrap();
    assert!(outcomes.is_empty());

    let code = report(&plain_terminal(), &outcomes).unwrap();
    assert_eq!(code, 0);
}

/// A check executor that only reports a verdict once *every* check in the suite
/// is running simultaneously: each run blocks on a shared [`Barrier`] sized to
/// the whole suite. If the pipeline executed checks one-at-a-time (a barrier
/// between phases, or a serialized actor handler), the barrier would never trip
/// and every run would time out into a no-verdict (errored) outcome. That all
/// checks instead come back satisfied is a direct demonstration that they
/// execute concurrently — i.e. a check starts the moment discovery streams it,
/// without waiting for the others.
struct InterleavingExecutor {
    barrier: Arc<Barrier>,
}

#[async_trait]
impl CheckExecutor for InterleavingExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        // Proceed only once all checks have reached this point concurrently.
        let interleaved = tokio::time::timeout(Duration::from_secs(5), self.barrier.wait())
            .await
            .is_ok();
        Ok(AgentOutcome {
            verdict: interleaved.then(|| CheckReport {
                success: true,
                evidence: Some(format!("check {} ran concurrently", req.check_id)),
            }),
            stop_reason: Some("interleaving probe".into()),
            turns: 1,
            error: None,
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checks_execute_concurrently_not_in_a_barrier() {
    // Two independent requirements (two checks total). The default concurrency
    // is 2, so both should run at once.
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("CHECKS.md"),
        "# Requirement One\ncheck one\n\n# Requirement Two\ncheck two\n",
    )
    .unwrap();
    let reqs = discover(dir.path()).await.unwrap();
    assert_eq!(reqs.len(), 2);

    let executor = Arc::new(InterleavingExecutor {
        barrier: Arc::new(Barrier::new(2)),
    });
    let cfg = configuration();
    assert_eq!(cfg.concurrency, 2, "test assumes a concurrency of 2");

    let outcomes = run_to_outcomes(
        &cfg,
        executor,
        Arc::new(NoopSandbox),
        dir.path(),
        &reqs,
        null_backend(),
    )
    .await
    .unwrap();

    // Both satisfied ⇒ both ran simultaneously (the barrier tripped).
    assert!(
        outcomes.iter().all(|o| o.satisfied),
        "checks did not interleave: {outcomes:?}"
    );
    let code = report(&plain_terminal(), &outcomes).unwrap();
    assert_eq!(code, 0);
}

#[tokio::test]
async fn invalid_suite_aborts_run_without_spawning_agents() {
    // An orphan `## Check` (no preceding requirement) makes the suite invalid.
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("CHECKS.md"),
        "## Check Orphan\nno requirement above me\n",
    )
    .unwrap();

    // Drive the *full* pipeline (with the discovery actor) so the strict
    // whole-run abort path is exercised end-to-end.
    let fake = Arc::new(FakeExecutor::new());
    let cfg = configuration();
    let result = run_pipeline(
        &cfg,
        fake.clone(),
        Arc::new(NoopSandbox),
        dir.path(),
        null_backend(),
    )
    .await;

    // The run aborts as an `Err` diagnostic (so CI distinguishes "tool errored"
    // from "checks failed"), and — crucially — no agent was ever spawned.
    let err = result.expect_err("an invalid suite must abort the run");
    assert!(format!("{err:?}").contains("orphan"), "got: {err:?}");
    assert!(
        fake.seen().is_empty(),
        "no agents should run for an invalid suite, saw: {:?}",
        fake.seen()
    );
}
