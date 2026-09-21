//! Orchestration-level tests for `multi plan` (MULTI-1824): caching,
//! concurrency-agnostic reuse, root refusal, and byte-identical output
//! regardless of scan directory. Drives [`run_with_planner`] directly with a
//! [`FakePlanner`] (or, for the acceptance bullet that spans both layers, a
//! real [`AgentPlanner`] over a [`FakeExecutor`] and a mock Jev server) —
//! calibration/decision-precedence itself is covered by `planner`'s own
//! tests.

use std::sync::Arc;

use clap::Parser;
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::checks::discovery::discover;
use crate::checks::executor::{FakeExecutor, ReadOnlyTool};
use crate::checks::jev::error::TYPESAFE_API_KEY_VAR;
use crate::checks::jev::plan_file::{AgentReason, Decider, JevCalibration, PlanCall, Reading};
use crate::checks::jev::replay::{self, CallChecksum};
use crate::checks::model::{Check, RootSource};
use crate::checks::sandbox::RecordingSandbox;
use crate::{Cli, Terminal};

/// A terminal with color forced off, mirroring `checks::e2e`'s own
/// `plain_terminal` helper. `run_with_planner`'s per-check output lines are
/// not asserted on directly in these tests (no test-friendly stdout capture
/// exists in this crate yet); the structural assertions below (exit code,
/// files written, `FakePlanner::seen()`) exercise the same behavior.
fn plain_terminal() -> Terminal {
    let cli = Cli::parse_from(["multi", "--enable-colors", "never"]);
    Terminal::new(&cli)
}

async fn read_checksum(root: &std::path::Path, relative: &str) -> String {
    let call = replay::replay_call(ReadOnlyTool::Read, &json!({"file_path": relative}), root).await;
    match call.checksums {
        Some(CallChecksum::Single(x)) => x,
        other => panic!("expected a single checksum, got {other:?}"),
    }
}

/// Build a `decider = jev`-planned entry for `check`, whose one `Read` call
/// names `relative_file` — its `xxh64` is computed fresh against `root`, so
/// the entry is fully consistent with whatever the file currently contains.
async fn planned_for(root: &std::path::Path, check: &Check, relative_file: &str) -> PlannedCheck {
    let xxh64 = read_checksum(root, relative_file).await;
    PlannedCheck {
        title: check.title.clone(),
        prompt_xxh64: plan_file::prompt_xxh64(&check.title, &check.prompt),
        decider: Decider::Jev,
        verdict: true,
        evidence: Some("looks fine".to_string()),
        jev: Some(JevCalibration {
            model: "jev-1.13.0".to_string(),
            noul: 0.95,
            control_noul: 0.02,
            reading: Reading::Satisfied,
        }),
        calls: vec![PlanCall::Read {
            input: json!({ "file_path": relative_file }),
            xxh64,
        }],
    }
}

fn write_two_check_fixture(dir: &std::path::Path) {
    std::fs::write(dir.join("MultiTool.toml"), "").unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(dir.join("src/b.rs"), "fn b() {}\n").unwrap();
    std::fs::write(
        dir.join("CHECKS.md"),
        "# Requirement Demo\n## Check A\nCheck A prompt\n## Check B\nCheck B prompt\n",
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Caching
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_run_plans_every_check_second_run_reuses_all() {
    let dir = TempDir::new().unwrap();
    write_two_check_fixture(dir.path());
    let reqs = discover(dir.path()).await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].checks.len(), 2);

    let planned_a = planned_for(dir.path(), &reqs[0].checks[0], "src/a.rs").await;
    let planned_b = planned_for(dir.path(), &reqs[0].checks[1], "src/b.rs").await;
    let fake1 = Arc::new(
        FakePlanner::new()
            .with_planned(0, planned_a)
            .with_planned(1, planned_b),
    );
    let term = plain_terminal();
    let code = run_with_planner(&term, &reqs, fake1.clone(), 2, false)
        .await
        .unwrap();
    assert_eq!(code, 0);
    let mut seen = fake1.seen();
    seen.sort_unstable();
    assert_eq!(seen, vec![0, 1], "first run plans every check");

    let plan = plan_file::PlanStore::load(dir.path())
        .unwrap()
        .expect("plan file written");
    assert_eq!(plan.requirements.len(), 1);
    assert_eq!(plan.requirements[0].checks.len(), 2);

    // Second run: nothing changed, so both checks reuse from cache — the
    // planner is invoked zero times.
    let reqs2 = discover(dir.path()).await.unwrap();
    let fake2 = Arc::new(FakePlanner::new());
    let code2 = run_with_planner(&term, &reqs2, fake2.clone(), 2, false)
        .await
        .unwrap();
    assert_eq!(code2, 0);
    assert!(
        fake2.seen().is_empty(),
        "an immediate second run invokes the planner zero times"
    );
}

#[tokio::test]
async fn editing_a_read_file_replans_only_the_affected_check() {
    let dir = TempDir::new().unwrap();
    write_two_check_fixture(dir.path());
    let reqs = discover(dir.path()).await.unwrap();
    let planned_a = planned_for(dir.path(), &reqs[0].checks[0], "src/a.rs").await;
    let planned_b = planned_for(dir.path(), &reqs[0].checks[1], "src/b.rs").await;
    let fake1 = Arc::new(
        FakePlanner::new()
            .with_planned(0, planned_a)
            .with_planned(1, planned_b),
    );
    let term = plain_terminal();
    run_with_planner(&term, &reqs, fake1, 2, false)
        .await
        .unwrap();

    // Edit the file check A (index 0) reads; check B's evidence is untouched.
    std::fs::write(dir.path().join("src/a.rs"), "fn a() { /* changed */ }\n").unwrap();

    let reqs2 = discover(dir.path()).await.unwrap();
    let replanned_a = planned_for(dir.path(), &reqs2[0].checks[0], "src/a.rs").await;
    let fake2 = Arc::new(FakePlanner::new().with_planned(0, replanned_a));
    let code = run_with_planner(&term, &reqs2, fake2.clone(), 2, false)
        .await
        .unwrap();
    assert_eq!(code, 0);
    assert_eq!(
        fake2.seen(),
        vec![0],
        "only the check whose evidence changed is replanned"
    );
}

#[tokio::test]
async fn editing_a_checks_prompt_replans_it() {
    let dir = TempDir::new().unwrap();
    write_two_check_fixture(dir.path());
    let reqs = discover(dir.path()).await.unwrap();
    let planned_a = planned_for(dir.path(), &reqs[0].checks[0], "src/a.rs").await;
    let planned_b = planned_for(dir.path(), &reqs[0].checks[1], "src/b.rs").await;
    let fake1 = Arc::new(
        FakePlanner::new()
            .with_planned(0, planned_a)
            .with_planned(1, planned_b),
    );
    let term = plain_terminal();
    run_with_planner(&term, &reqs, fake1, 2, false)
        .await
        .unwrap();

    // Edit check B's prompt text only — its evidence file is untouched, but
    // `prompt_xxh64` no longer matches the stored entry.
    std::fs::write(
        dir.path().join("CHECKS.md"),
        "# Requirement Demo\n## Check A\nCheck A prompt\n## Check B\nCheck B prompt CHANGED\n",
    )
    .unwrap();

    let reqs2 = discover(dir.path()).await.unwrap();
    assert_ne!(reqs2[0].checks[1].prompt, reqs[0].checks[1].prompt);
    let replanned_b = planned_for(dir.path(), &reqs2[0].checks[1], "src/b.rs").await;
    let fake2 = Arc::new(FakePlanner::new().with_planned(1, replanned_b));
    let code = run_with_planner(&term, &reqs2, fake2.clone(), 2, false)
        .await
        .unwrap();
    assert_eq!(code, 0);
    assert_eq!(
        fake2.seen(),
        vec![1],
        "only the check whose prompt changed is replanned"
    );
}

#[tokio::test]
async fn force_replans_every_check_even_when_fresh() {
    let dir = TempDir::new().unwrap();
    write_two_check_fixture(dir.path());
    let reqs = discover(dir.path()).await.unwrap();
    let planned_a = planned_for(dir.path(), &reqs[0].checks[0], "src/a.rs").await;
    let planned_b = planned_for(dir.path(), &reqs[0].checks[1], "src/b.rs").await;
    let fake1 = Arc::new(
        FakePlanner::new()
            .with_planned(0, planned_a.clone())
            .with_planned(1, planned_b.clone()),
    );
    let term = plain_terminal();
    run_with_planner(&term, &reqs, fake1, 2, false)
        .await
        .unwrap();

    let reqs2 = discover(dir.path()).await.unwrap();
    let fake2 = Arc::new(
        FakePlanner::new()
            .with_planned(0, planned_a)
            .with_planned(1, planned_b),
    );
    let code = run_with_planner(&term, &reqs2, fake2.clone(), 2, true)
        .await
        .unwrap();
    assert_eq!(code, 0);
    let mut seen = fake2.seen();
    seen.sort_unstable();
    assert_eq!(
        seen,
        vec![0, 1],
        "--force bypasses the cache for every check"
    );
}

/// MULTI-1824 acceptance: a check with truncated discovery is `reused` (zero
/// planner invocations) on an immediate second run — `classify_call` excludes
/// a call from freshness only when *both* the stored and replayed sides are
/// truncated, which this fixture (260 files matching one `Grep`) reproduces
/// for real, rather than asserting the rule in isolation (see `replay`'s own
/// tests for that).
#[tokio::test]
async fn truncated_discovery_entry_is_reused_not_replanned() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("MultiTool.toml"), "").unwrap();
    for i in 0..260 {
        std::fs::write(dir.path().join(format!("f{i:04}.txt")), "NEEDLE\n").unwrap();
    }
    std::fs::write(
        dir.path().join("CHECKS.md"),
        "# Requirement Search\n## Check Grep\nfind needle\n",
    )
    .unwrap();

    let reqs = discover(dir.path()).await.unwrap();
    let check = &reqs[0].checks[0];
    let planned = PlannedCheck {
        title: check.title.clone(),
        prompt_xxh64: plan_file::prompt_xxh64(&check.title, &check.prompt),
        decider: Decider::Agent(AgentReason::TruncatedDiscovery),
        verdict: true,
        evidence: Some("many matches".to_string()),
        jev: None,
        calls: vec![PlanCall::Truncated {
            tool: ReadOnlyTool::Grep,
            input: json!({ "pattern": "NEEDLE", "path": "." }),
        }],
    };

    let fake1 = Arc::new(FakePlanner::new().with_planned(0, planned));
    let term = plain_terminal();
    let code = run_with_planner(&term, &reqs, fake1.clone(), 1, false)
        .await
        .unwrap();
    assert_eq!(code, 0);
    assert_eq!(fake1.seen(), vec![0]);

    let reqs2 = discover(dir.path()).await.unwrap();
    let fake2 = Arc::new(FakePlanner::new());
    let code2 = run_with_planner(&term, &reqs2, fake2.clone(), 1, false)
        .await
        .unwrap();
    assert_eq!(code2, 0);
    assert!(
        fake2.seen().is_empty(),
        "a truncated entry is reused, not replanned, on an unchanged tree"
    );
}

/// The summary line's truncated-call detection, unit-tested directly: a
/// `Reused` or `Planned` outcome carrying a `PlanCall::Truncated` counts,
/// an `Error` outcome never does (it has no entry to inspect).
#[test]
fn entry_has_truncated_call_detects_either_outcome_kind() {
    let truncated_call = PlanCall::Truncated {
        tool: ReadOnlyTool::Grep,
        input: json!({}),
    };
    let reused = LineOutcome::Reused(plan_file::PlanCheck {
        title: "t".to_string(),
        ordinal: 0,
        prompt_xxh64: "0".repeat(16),
        decider: Decider::Agent(AgentReason::TruncatedDiscovery),
        verdict: true,
        evidence: None,
        jev: None,
        calls: vec![truncated_call.clone()],
    });
    assert!(entry_has_truncated_call(&reused));

    let planned = LineOutcome::Planned(PlannedCheck {
        title: "t".to_string(),
        prompt_xxh64: "0".repeat(16),
        decider: Decider::Jev,
        verdict: true,
        evidence: None,
        jev: None,
        calls: vec![],
    });
    assert!(!entry_has_truncated_call(&planned));

    assert!(!entry_has_truncated_call(&LineOutcome::Error(
        miette::miette!("boom")
    )));
}

// ---------------------------------------------------------------------------
// Root refusal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn requirements_file_without_a_manifest_is_refused_other_files_still_planned() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("good/src")).unwrap();
    std::fs::write(dir.path().join("good/MultiTool.toml"), "").unwrap();
    std::fs::write(dir.path().join("good/src/ok.rs"), "fn ok() {}\n").unwrap();
    std::fs::write(
        dir.path().join("good/CHECKS.md"),
        "# Requirement Good\n## Check OK\nprompt\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("bad")).unwrap();
    std::fs::write(
        dir.path().join("bad/CHECKS.md"),
        "# Requirement Bad\n## Check Nope\nprompt\n",
    )
    .unwrap();

    let reqs = discover(dir.path()).await.unwrap();
    assert_eq!(reqs.len(), 2);
    let good_req = reqs.iter().find(|r| r.title == "Good").unwrap();
    assert_eq!(good_req.root_source, RootSource::Manifest);
    let bad_req = reqs.iter().find(|r| r.title == "Bad").unwrap();
    assert_eq!(bad_req.root_source, RootSource::ScanDirectory);

    let planned_ok = planned_for(good_req.root.as_path(), &good_req.checks[0], "src/ok.rs").await;
    let fake = Arc::new(FakePlanner::new().with_planned(0, planned_ok));
    let term = plain_terminal();
    let code = run_with_planner(&term, &reqs, fake.clone(), 2, false)
        .await
        .unwrap();

    assert_eq!(code, 1, "the refused file makes the run exit non-zero");
    assert_eq!(
        fake.seen(),
        vec![0],
        "only the good file's check ever reaches the planner"
    );

    assert!(
        plan_file::PlanStore::load(&dir.path().join("good"))
            .unwrap()
            .is_some(),
        "the manifest-rooted directory still gets a plan"
    );
    assert!(
        plan_file::PlanStore::load(&dir.path().join("bad"))
            .unwrap()
            .is_none(),
        "no plan file is written for the refused directory"
    );
}

// ---------------------------------------------------------------------------
// Invocation-independence
// ---------------------------------------------------------------------------

/// MULTI-1824 acceptance: planning from the repository root and from a
/// subdirectory produce byte-identical plan entries.
#[tokio::test]
async fn planning_from_repo_root_or_a_subdirectory_is_byte_identical() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("MultiTool.toml"), "").unwrap();
    std::fs::create_dir_all(dir.path().join("services/keystore/src")).unwrap();
    std::fs::write(
        dir.path().join("services/keystore/src/sign.rs"),
        "fn sign() {}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("services/keystore/CHECKS.md"),
        "# Requirement Keystore\n## Check Sign\nverify signing\n",
    )
    .unwrap();

    let term = plain_terminal();
    let plan_path = dir.path().join("services/keystore/.check-plan.toml");

    // From the repository root.
    let reqs_root = discover(dir.path()).await.unwrap();
    let planned = planned_for(
        reqs_root[0].root.as_path(),
        &reqs_root[0].checks[0],
        "services/keystore/src/sign.rs",
    )
    .await;
    let fake_root = Arc::new(FakePlanner::new().with_planned(0, planned.clone()));
    run_with_planner(&term, &reqs_root, fake_root, 1, false)
        .await
        .unwrap();
    let from_root = std::fs::read_to_string(&plan_path).unwrap();

    // From the subdirectory the requirements file itself lives in.
    std::fs::remove_file(&plan_path).unwrap();
    let reqs_sub = discover(&dir.path().join("services/keystore"))
        .await
        .unwrap();
    let fake_sub = Arc::new(FakePlanner::new().with_planned(0, planned));
    run_with_planner(&term, &reqs_sub, fake_sub, 1, false)
        .await
        .unwrap();
    let from_sub = std::fs::read_to_string(&plan_path).unwrap();

    assert_eq!(from_root, from_sub);
}

// ---------------------------------------------------------------------------
// Abort on a credential/request-level Jev failure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn abort_worthy_planner_error_aborts_the_run_and_writes_nothing() {
    let dir = TempDir::new().unwrap();
    write_two_check_fixture(dir.path());
    let reqs = discover(dir.path()).await.unwrap();

    let fake = Arc::new(
        FakePlanner::new()
            .with_abort(0, "missing key")
            .with_planned(
                1,
                planned_for(dir.path(), &reqs[0].checks[1], "src/b.rs").await,
            ),
    );
    let term = plain_terminal();
    let err = run_with_planner(&term, &reqs, fake, 2, false)
        .await
        .expect_err("an abort-worthy Jev error must fail the whole run");
    assert!(err.downcast_ref::<planner::AbortPlanRun>().is_some());
    assert!(
        plan_file::PlanStore::load(dir.path()).unwrap().is_none(),
        "no plan file is written when the run aborts"
    );
}

// ---------------------------------------------------------------------------
// Zero Jev calls on a reused entry (real AgentPlanner + mock Jev)
// ---------------------------------------------------------------------------

static API_KEY_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn with_api_key<F, Fut, T>(body: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let _guard = API_KEY_ENV_LOCK.lock().await;
    let previous = std::env::var(TYPESAFE_API_KEY_VAR).ok();
    // SAFETY: serialized by `API_KEY_ENV_LOCK`; see `client.rs`'s own
    // `with_api_key` for the identical justification.
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

/// MULTI-1824 acceptance: a `reused` entry makes zero Jev calls (no verdict
/// call, no control call). Drives a *real* [`AgentPlanner`] over a
/// [`FakeExecutor`] and a wiremock Jev server: the first run plans (and
/// necessarily calls Jev twice), the second run — over an unchanged tree —
/// reuses, and the mock server's received-request count must not move.
#[tokio::test]
async fn reused_entry_makes_zero_jev_calls() {
    let dir = TempDir::new().unwrap();
    write_two_check_fixture(dir.path());

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_string_contains("\"evidence\":[]"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": { "satisfied": {"type": "noul", "noul": 0.02} },
            "usage": {"input_tokens": 10, "output_tokens": 5},
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": { "satisfied": {"type": "noul", "noul": 0.95} },
            "usage": {"input_tokens": 200, "output_tokens": 20},
        })))
        .mount(&server)
        .await;

    let client = Arc::new(crate::checks::jev::client::JevClient::new(server.uri()).unwrap());
    let cfg = crate::checks::config::JevConfig {
        model: "jev-latest".to_string(),
        threshold: 0.75,
        base_url: server.uri(),
    };

    let reqs = discover(dir.path()).await.unwrap();
    let calls_a = vec![crate::checks::executor::ToolCall {
        tool: ReadOnlyTool::Read,
        input: json!({ "file_path": dir.path().join("src/a.rs").to_string_lossy() }),
    }];
    let calls_b = vec![crate::checks::executor::ToolCall {
        tool: ReadOnlyTool::Read,
        input: json!({ "file_path": dir.path().join("src/b.rs").to_string_lossy() }),
    }];
    let executor = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("ok"))
            .with_tool_calls(0, calls_a)
            .with_report(1, true, Some("ok"))
            .with_tool_calls(1, calls_b),
    );
    let sandbox = Arc::new(RecordingSandbox::new());
    let real_planner: Arc<dyn Planner + Send + Sync> =
        Arc::new(AgentPlanner::new(executor.clone(), sandbox, client, cfg, 3));
    let term = plain_terminal();

    with_api_key(|| run_with_planner(&term, &reqs, real_planner.clone(), 2, false))
        .await
        .unwrap();
    let after_first_run = server.received_requests().await.unwrap().len();
    assert!(after_first_run > 0, "planning must call Jev");

    let reqs2 = discover(dir.path()).await.unwrap();
    with_api_key(|| run_with_planner(&term, &reqs2, real_planner, 2, false))
        .await
        .unwrap();
    let after_second_run = server.received_requests().await.unwrap().len();

    assert_eq!(
        after_first_run, after_second_run,
        "a fully-reused second run makes no additional Jev calls"
    );
    // No additional agent runs either.
    assert_eq!(executor.seen().len(), 2, "no re-run agents on reuse");
}
