//! End-to-end pipeline test (MULTI-1355; actor pipeline in MULTI-1368).
//!
//! Drives the real discovery → execution → reporting → exit-code actor pipeline
//! over a fixture tree, with the fake executor and a no-op sandbox injected so
//! verdicts are scripted. Runs deterministically in CI with no `claude`,
//! network, or APFS dependency, guarding the end-to-end contract against
//! regressions.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use clap::Parser;
use miette::Result;
use tempfile::TempDir;
use tokio::sync::Barrier;

use crate::checks::config::{Config, configuration};
use crate::checks::discovery::discover;
use crate::checks::executor::{
    AgentOutcome, AgentRunRequest, CheckExecutor, CheckReport, FakeExecutor,
};
use crate::checks::model::RootSource;
use crate::checks::presenter::null_backend;
use crate::checks::reporting::report;
use crate::checks::sandbox::{NoopSandbox, RecordingSandbox};
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
    let outcomes = run_to_outcomes(&cfg, fake, Arc::new(NoopSandbox), &reqs, null_backend())
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
            trace_jsonl: None,
            tool_calls: Vec::new(),
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checks_execute_concurrently_not_in_a_barrier() {
    // Two independent requirements (two checks total). Pin concurrency to 2
    // explicitly so both run at once regardless of the host's core count (the
    // default now tracks available parallelism, not a fixed value).
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
    let cfg = Config {
        concurrency: 2,
        ..configuration()
    };

    let outcomes = run_to_outcomes(&cfg, executor, Arc::new(NoopSandbox), &reqs, null_backend())
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
        None,
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

/// A fixture with a `MultiTool.toml` at the repository root and a requirements
/// file nested several levels down under a service directory — the monorepo
/// shape MULTI-1834 targets.
fn write_manifest_fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("MultiTool.toml"), "").unwrap();
    fs::create_dir_all(dir.path().join("services/keystore")).unwrap();
    fs::write(
        dir.path().join("services/keystore/CHECKS.md"),
        "# Requirement Keystore Scoped\ndo it\n",
    )
    .unwrap();
    dir
}

/// MULTI-1834 acceptance: scanning a *subdirectory* of a repository with a
/// root manifest still clones the repository root, not the scanned
/// subdirectory — root resolution is per requirements file, never the scan
/// directory.
#[tokio::test]
async fn sandbox_clones_repository_root_not_scan_directory() {
    let dir = write_manifest_fixture();
    let scan_dir = dir.path().join("services/keystore");

    let reqs = discover(&scan_dir).await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].root, dir.path());

    let sandbox = Arc::new(RecordingSandbox::new());
    let cfg = configuration();
    let fake = Arc::new(FakeExecutor::new().with_report(0, true, Some("ok")));
    let outcomes = run_to_outcomes(&cfg, fake, sandbox.clone(), &reqs, null_backend())
        .await
        .unwrap();
    assert!(outcomes[0].satisfied);

    // The sandbox cloned the repository root, not the scanned subdirectory.
    assert_eq!(sandbox.sources(), vec![dir.path().to_path_buf()]);
}

/// MULTI-1834 acceptance: scanning from the repository root itself is
/// unchanged — the sandbox still clones that same root.
#[tokio::test]
async fn sandbox_clones_repository_root_when_scanning_from_root() {
    let dir = write_manifest_fixture();

    let reqs = discover(dir.path()).await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].root, dir.path());

    let sandbox = Arc::new(RecordingSandbox::new());
    let cfg = configuration();
    let fake = Arc::new(FakeExecutor::new().with_report(0, true, Some("ok")));
    let outcomes = run_to_outcomes(&cfg, fake, sandbox.clone(), &reqs, null_backend())
        .await
        .unwrap();
    assert!(outcomes[0].satisfied);
    assert_eq!(sandbox.sources(), vec![dir.path().to_path_buf()]);
}

/// MULTI-1834 acceptance: a retried check clones the repository root **once
/// per attempt**, not once for the whole check.
#[tokio::test]
async fn sandbox_clones_repository_root_once_per_attempt() {
    let dir = write_manifest_fixture();
    let scan_dir = dir.path().join("services/keystore");
    let reqs = discover(&scan_dir).await.unwrap();

    let sandbox = Arc::new(RecordingSandbox::new());
    let cfg = configuration(); // max_attempts = 3
    // Silent on attempt 1, reports on attempt 2 — exercises the retry path.
    let fake = Arc::new(FakeExecutor::new().with_silent_until(0, 2, true, Some("ok")));
    let outcomes = run_to_outcomes(&cfg, fake, sandbox.clone(), &reqs, null_backend())
        .await
        .unwrap();
    assert!(outcomes[0].satisfied);

    // Each attempt gets its own sandbox clone, and both clone the repository
    // root — not the scan directory.
    assert_eq!(
        sandbox.sources(),
        vec![dir.path().to_path_buf(), dir.path().to_path_buf()]
    );
}

/// Code-review regression for MULTI-1834: all the tests above pass an
/// **absolute** `TempDir` scan path, but `multi check`'s default scan
/// directory is the RELATIVE `.`, and the ticket's own motivating case —
/// `multi check services/keystore` — is a relative subdirectory argument
/// too. Before this fix, `Path::ancestors()` on a relative path never climbs
/// above itself, so root resolution silently fell back to the scan
/// directory: exactly the invocation-dependence this ticket exists to
/// remove. Drives discovery from a RELATIVE scan path and asserts the root
/// still resolves to the manifest above it, and `declared_in` is
/// root-relative (not an absolute host path).
#[allow(clippy::result_large_err)] // `figment::Jail`'s closure returns a large `Result`.
#[test]
fn relative_scan_path_still_resolves_the_repository_root() {
    figment::Jail::expect_with(|jail| {
        jail.create_file("MultiTool.toml", "")?;
        fs::create_dir_all("services/keystore").unwrap();
        fs::write(
            "services/keystore/CHECKS.md",
            "# Requirement Keystore Scoped\ndo it\n",
        )
        .unwrap();

        let repo_root = jail.directory().to_path_buf();

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            // A RELATIVE scan path, exactly `multi check services/keystore`
            // run from the repository root.
            let reqs = discover(Path::new("services/keystore")).await.unwrap();
            assert_eq!(reqs.len(), 1);
            assert_eq!(reqs[0].root, repo_root);
            assert_eq!(reqs[0].root_source, RootSource::Manifest);

            let sandbox = Arc::new(RecordingSandbox::new());
            let cfg = configuration();
            let fake = Arc::new(FakeExecutor::new().with_report(0, true, Some("ok")));
            let outcomes =
                run_to_outcomes(&cfg, fake.clone(), sandbox.clone(), &reqs, null_backend())
                    .await
                    .unwrap();
            assert!(outcomes[0].satisfied);

            // The sandbox cloned the repository root — not the relative scan
            // directory.
            assert_eq!(sandbox.sources(), vec![repo_root.clone()]);
            // The instructions state where the requirement was declared,
            // root-relative — never an absolute host path.
            assert_eq!(
                fake.declared_ins(),
                vec![PathBuf::from("services/keystore/CHECKS.md")]
            );
        });

        Ok(())
    });
}

/// Code-review regression for MULTI-1834, the companion invocation to the
/// test above: `cd services/keystore && multi check` scans `.` from *inside*
/// the subdirectory, with the manifest two levels above `cwd`. Root
/// resolution must still find it — a relative scan directory can't rely on
/// climbing from the scan root; it has to climb from the file's own
/// location — and it must resolve to the exact same root and `declared_in`
/// as scanning the subdirectory from the repository root (the test above),
/// proving resolution really is independent of the invocation directory.
#[allow(clippy::result_large_err)] // `figment::Jail`'s closure returns a large `Result`.
#[test]
fn cwd_inside_a_subdirectory_scanning_dot_still_finds_the_root_above() {
    figment::Jail::expect_with(|jail| {
        jail.create_file("MultiTool.toml", "")?;
        fs::create_dir_all("services/keystore").unwrap();
        fs::write(
            "services/keystore/CHECKS.md",
            "# Requirement Keystore Scoped\ndo it\n",
        )
        .unwrap();

        let repo_root = jail.directory().to_path_buf();

        // `cd services/keystore && multi check` — cwd moves two levels below
        // the manifest, and the scan directory is `.`.
        jail.change_dir("services/keystore")?;

        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let reqs = discover(Path::new(".")).await.unwrap();
            assert_eq!(reqs.len(), 1);
            assert_eq!(reqs[0].root, repo_root);
            assert_eq!(reqs[0].root_source, RootSource::Manifest);

            let sandbox = Arc::new(RecordingSandbox::new());
            let cfg = configuration();
            let fake = Arc::new(FakeExecutor::new().with_report(0, true, Some("ok")));
            let outcomes =
                run_to_outcomes(&cfg, fake.clone(), sandbox.clone(), &reqs, null_backend())
                    .await
                    .unwrap();
            assert!(outcomes[0].satisfied);
            assert_eq!(sandbox.sources(), vec![repo_root.clone()]);
            // Same root-relative `declared_in` as the previous test, despite
            // the discovered file's own path being spelled differently
            // (`./CHECKS.md` here vs. `services/keystore/CHECKS.md` there).
            assert_eq!(
                fake.declared_ins(),
                vec![PathBuf::from("services/keystore/CHECKS.md")]
            );
        });

        Ok(())
    });
}

/// A check executor that decides every check from
/// [`AgentRunRequest::source_dir`] alone and never calls
/// `AgentRunRequest::sandbox.acquire()` — standing in for MULTI-1825's Jev
/// decision path, which replays evidence in-host and only falls back to an
/// agent (and its sandbox) when Jev can't decide.
struct NeverAcquiresExecutor;

#[async_trait]
impl CheckExecutor for NeverAcquiresExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        // The source directory is available without ever touching the lease.
        assert!(req.source_dir.exists());
        Ok(AgentOutcome {
            verdict: Some(CheckReport {
                success: true,
                evidence: Some("decided from source_dir, no sandbox needed".into()),
            }),
            stop_reason: Some("never-acquires probe".into()),
            turns: 0,
            error: None,
            trace_jsonl: None,
            tool_calls: Vec::new(),
        })
    }
}

/// MULTI-1818 acceptance: an executor that never acquires its sandbox lease
/// causes zero `Sandbox::create` calls — the clone is created lazily, on
/// demand, rather than eagerly for every check like before this ticket.
#[tokio::test]
async fn executor_that_never_acquires_the_lease_creates_no_sandbox() {
    let dir = write_manifest_fixture();
    let scan_dir = dir.path().join("services/keystore");
    let reqs = discover(&scan_dir).await.unwrap();

    let sandbox = Arc::new(RecordingSandbox::new());
    let cfg = configuration();
    let outcomes = run_to_outcomes(
        &cfg,
        Arc::new(NeverAcquiresExecutor),
        sandbox.clone(),
        &reqs,
        null_backend(),
    )
    .await
    .unwrap();
    assert!(outcomes[0].satisfied);

    assert!(
        sandbox.sources().is_empty(),
        "expected zero Sandbox::create calls, got {:?}",
        sandbox.sources()
    );
}
