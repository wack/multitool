//! End-to-end pipeline test (MULTI-1355).
//!
//! Drives the real discovery → execution → reporting → exit-code pipeline over a
//! fixture tree, with the fake executor and a no-op sandbox injected so verdicts
//! are scripted. Runs deterministically in CI with no `claude`, network, or APFS
//! dependency, guarding the end-to-end contract against regressions.

use std::fs;
use std::sync::Arc;

use clap::Parser;
use tempfile::TempDir;

use crate::checks::config::configuration;
use crate::checks::discovery::discover;
use crate::checks::execution::execute;
use crate::checks::executor::FakeExecutor;
use crate::checks::reporting::report;
use crate::checks::sandbox::NoopSandbox;
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
    let outcomes = execute(&cfg, fake.clone(), Arc::new(NoopSandbox), dir.path(), &reqs)
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
    let outcomes = execute(&cfg, fake, Arc::new(NoopSandbox), dir.path(), &reqs)
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
    let outcomes = execute(
        &cfg,
        Arc::new(FakeExecutor::new()),
        Arc::new(NoopSandbox),
        dir.path(),
        &reqs,
    )
    .await
    .unwrap();
    assert!(outcomes.is_empty());

    let code = report(&plain_terminal(), &outcomes).unwrap();
    assert_eq!(code, 0);
}
