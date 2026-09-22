//! Table-driven tests for [`super::JevExecutor`] — one case per decision-table
//! row (MULTI-1825's own module docs), plus the error-kind and retry
//! contracts. Drives the real `JevExecutor` over a [`FakeExecutor`] inner, a
//! [`RecordingSandbox`], a `wiremock` Jev server, and `.check-plan.toml`
//! files written via [`PlanStore`] in a tempdir — never a real agent, never a
//! real TypeSafe request.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::checks::config::JevConfig;
use crate::checks::executor::{FakeExecutor, PlanIdentity, ReadOnlyTool, ToolCall};
use crate::checks::jev::error::TYPESAFE_API_KEY_VAR;
use crate::checks::jev::plan_file::{PlanCall, PlanFile, PlanRequirement, Reading};
use crate::checks::jev::replay;
use crate::checks::model::Check;
use crate::checks::sandbox::RecordingSandbox;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn check() -> Check {
    Check {
        title: "No yellow".to_string(),
        prompt: "scan for yellow text".to_string(),
    }
}

fn jev_config(threshold: f64, base_url: &str) -> JevConfig {
    JevConfig {
        model: "jev-latest".to_string(),
        threshold,
        base_url: base_url.to_string(),
    }
}

fn write_file(dir: &Path, relative: &str, content: &str) {
    let p = dir.join(relative);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(p, content).unwrap();
}

/// Compute the `xxh64` a fresh `Read` of `relative` (against `root`) would
/// checksum to right now — used to freeze a `PlanCall::Read` that starts out
/// `Fresh`.
async fn read_checksum(root: &Path, relative: &str) -> String {
    let call = replay::replay_call(ReadOnlyTool::Read, &json!({"file_path": relative}), root).await;
    match call.checksums {
        Some(replay::CallChecksum::Single(x)) => x,
        other => panic!("expected a single checksum, got {other:?}"),
    }
}

/// Write a one-requirement, one-check `.check-plan.toml` beside `dir`.
fn write_plan(dir: &Path, entry: PlanCheck) {
    let plan = PlanFile::new(vec![PlanRequirement {
        title: "R".to_string(),
        source: "CHECKS.md".to_string(),
        ordinal: 0,
        checks: vec![entry],
    }]);
    PlanStore::write(dir, &plan).unwrap();
}

fn plan_entry(
    c: &Check,
    decider: Decider,
    verdict: bool,
    evidence: Option<&str>,
    jev: Option<JevCalibration>,
    calls: Vec<PlanCall>,
) -> PlanCheck {
    PlanCheck {
        title: c.title.clone(),
        ordinal: 0,
        prompt_xxh64: plan_file::prompt_xxh64(&c.title, &c.prompt),
        decider,
        verdict,
        evidence: evidence.map(str::to_string),
        jev,
        calls,
    }
}

/// A calibration that agrees with `verdict` under `threshold = 0.75` (the
/// default every test uses unless it's specifically exercising the
/// raised-threshold row).
fn calibration(model: &str, noul: f64) -> JevCalibration {
    JevCalibration {
        model: model.to_string(),
        noul,
        control_noul: 0.02,
        reading: Some(Reading::Satisfied),
    }
}

/// A `PlanIdentity` naming `dir`'s `.check-plan.toml`, requirement/check 0.
fn plan_identity(dir: &Path) -> PlanIdentity {
    PlanIdentity {
        dir: dir.to_path_buf(),
        source: "CHECKS.md".to_string(),
        req_ordinal: 0,
        check_ordinal: 0,
        requirement_title: "R".to_string(),
        root_source: RootSource::Manifest,
    }
}

fn request(
    root: &Path,
    sandbox: Arc<dyn crate::checks::sandbox::Sandbox + Send + Sync>,
    plan: PlanIdentity,
    attempt: u32,
) -> AgentRunRequest {
    AgentRunRequest {
        check_id: 0,
        check: check(),
        source_dir: root.to_path_buf(),
        sandbox: crate::checks::sandbox::SandboxLease::new(sandbox, root.to_path_buf()),
        declared_in: PathBuf::from("CHECKS.md"),
        attempt,
        progress: None,
        plan,
    }
}

static API_KEY_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Run async `body` with `TYPESAFE_API_KEY` set for its duration, restoring
/// whatever was there before — mirrors `plan::planner`'s own test helper of
/// the same name.
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

/// Mount a single scripted Jev answer (the live `multi check` consult never
/// sends a negative control — that's `multi plan`-only).
async fn mock_verify(server: &MockServer, model: &str, noul: f64) {
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": model,
            "answers": {
                "satisfied": {"type": "noul", "noul": noul},
            },
            "usage": {"input_tokens": 50, "output_tokens": 10},
        })))
        .mount(server)
        .await;
}

fn executor(
    inner: Arc<FakeExecutor>,
    jev_client: JevClient,
    jev_config: JevConfig,
    no_cache: bool,
) -> JevExecutor {
    JevExecutor::new(inner, Arc::new(jev_client), jev_config, no_cache, false)
}

/// Like [`executor`], but with `--frozen` set — for the self-healing tests
/// that need to assert it disables healing entirely (MULTI-1826).
fn frozen_executor(
    inner: Arc<FakeExecutor>,
    jev_client: JevClient,
    jev_config: JevConfig,
) -> JevExecutor {
    JevExecutor::new(inner, Arc::new(jev_client), jev_config, false, true)
}

// ---------------------------------------------------------------------------
// Row: no entry / hash mismatch / decider = agent ⇒ Agent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_plan_file_at_all_decides_agent() {
    let dir = TempDir::new().unwrap();
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("agent said so")));
    let exec = executor(
        inner.clone(),
        JevClient::new("https://unused.invalid").unwrap(),
        jev_config(0.75, "https://unused.invalid"),
        false,
    );

    let req = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
    let outcome = exec.run_check(req).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
    assert_eq!(
        sandbox.sources().len(),
        1,
        "the agent path acquires a sandbox"
    );
}

#[tokio::test]
async fn prompt_hash_mismatch_decides_agent() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    // The stored entry's `prompt_xxh64` is for a DIFFERENT check body, so
    // `PlanFile::lookup` treats this exactly like "no entry".
    let mut entry = plan_entry(
        &check(),
        Decider::Jev,
        true,
        Some("old"),
        Some(calibration("jev-1.13.0", 0.9)),
        vec![PlanCall::Read {
            input: json!({"file_path": "src/a.rs"}),
            xxh64,
        }],
    );
    entry.prompt_xxh64 = plan_file::prompt_xxh64("a different title", "a different prompt");
    write_plan(dir.path(), entry);

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, None));
    let exec = executor(
        inner.clone(),
        JevClient::new("https://unused.invalid").unwrap(),
        jev_config(0.75, "https://unused.invalid"),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = exec.run_check(req).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
}

#[tokio::test]
async fn decider_agent_entry_decides_agent_with_zero_jev_calls() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    let entry = plan_entry(
        &check(),
        Decider::Agent(crate::checks::jev::plan_file::AgentReason::JevUncertain),
        true,
        Some("old"),
        None,
        vec![PlanCall::Read {
            input: json!({"file_path": "src/a.rs"}),
            xxh64,
        }],
    );
    write_plan(dir.path(), entry);

    let server = MockServer::start().await;
    // No mock mounted: any HTTP call would fail loudly (connection refused
    // is fine — wiremock just 404s an unmatched request, but we assert the
    // received-request count is zero below regardless).
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, None));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// Row: no manifest-derived root ⇒ Agent; no plan read; one warn per file
// ---------------------------------------------------------------------------

/// A minimal [`tracing::Subscriber`] that counts `WARN` events emitted from
/// this module — mirrors `crate::checks::jev::replay`'s own `WarnCounter`
/// test helper (no shared log-assertion helper exists elsewhere in this
/// crate).
#[derive(Clone, Default)]
struct WarnCounter(Arc<std::sync::atomic::AtomicUsize>);

impl WarnCounter {
    fn count(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl tracing::Subscriber for WarnCounter {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::WARN
            && event.metadata().target().contains("checks::jev::executor")
        {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

#[tokio::test]
async fn no_manifest_derived_root_decides_agent_no_plan_read_warns_once_per_file() {
    let dir = TempDir::new().unwrap();
    // A plan DOES exist on disk, to prove it is never even opened: if
    // `JevExecutor` read it, this test would settle `Cached`/`Jev` instead
    // of `Agent`.
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("cached"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );

    let mut scan_directory_plan = plan_identity(dir.path());
    scan_directory_plan.root_source = RootSource::ScanDirectory;

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("agent"))
            .with_report(1, true, Some("agent")),
    );
    let exec = executor(
        inner.clone(),
        JevClient::new("https://unused.invalid").unwrap(),
        jev_config(0.75, "https://unused.invalid"),
        false,
    );

    let counter = WarnCounter::default();
    let (first, second) = {
        let _guard = tracing::subscriber::set_default(counter.clone());
        let mut req0 = request(dir.path(), sandbox.clone(), scan_directory_plan.clone(), 1);
        req0.check_id = 0;
        let first = exec.run_check(req0).await.unwrap();

        // A second check under the SAME requirements file (same `dir`) must
        // not warn again.
        let mut req1 = request(dir.path(), sandbox.clone(), scan_directory_plan, 1);
        req1.check_id = 1;
        let second = exec.run_check(req1).await.unwrap();
        (first, second)
    };

    assert_eq!(first.decided_by, DecidedBy::Agent);
    assert_eq!(second.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0, 1]);
    assert_eq!(
        counter.count(),
        1,
        "warns exactly once per requirements file"
    );
}

// ---------------------------------------------------------------------------
// Row: Fresh ⇒ Cached — zero Jev calls, zero agent runs, zero sandbox creates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fresh_evidence_settles_cached_with_zero_calls_and_zero_sandbox() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("frozen evidence explanation"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );

    let server = MockServer::start().await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("never called")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
    let outcome = exec.run_check(req).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Cached);
    assert!(outcome.verdict.as_ref().unwrap().success);
    assert_eq!(
        outcome.verdict.as_ref().unwrap().evidence.as_deref(),
        Some("frozen evidence explanation")
    );
    assert_eq!(outcome.turns, 0);
    assert!(outcome.tool_calls.is_empty());
    assert!(inner.seen().is_empty(), "the agent must never run");
    assert!(sandbox.sources().is_empty(), "no sandbox on a Cached row");
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn no_cache_forces_fresh_evidence_through_a_live_jev_call() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("frozen"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );

    let server = MockServer::start().await;
    mock_verify(&server, "jev-1.13.0", 0.95).await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, None));
    // `no_cache: true` — file content is byte-identical (still `Fresh` on
    // replay), but the flag forces it to be treated as `ReadsStale`.
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        true,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Jev);
    assert!(inner.seen().is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Row: ReadsStale, Jev Satisfied ⇒ Jev (satisfied) — including a stored FAIL
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reads_stale_satisfied_settles_jev_with_zero_agent_and_zero_sandbox() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("stale"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    // Edit the file AFTER freezing the checksum: replay now reports
    // `ReadsStale` (content changed, same file).
    write_file(dir.path(), "src/a.rs", "fn a() { /* changed */ }\n");

    let server = MockServer::start().await;
    mock_verify(&server, "jev-1.13.0", 0.95).await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, None));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Jev);
    assert!(outcome.verdict.as_ref().unwrap().success);
    assert!(inner.seen().is_empty());
    assert!(sandbox.sources().is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// The subtlety the ticket calls out explicitly: a plan entry whose STORED
/// verdict is FAIL, under `ReadsStale`, whose live Jev call answers
/// `Satisfied` — the table is followed literally (`Satisfied` ⇒ satisfied),
/// not "confirm the old FAIL". The fresh evidence changed (that's what
/// `ReadsStale` means), and the live "is this satisfied" question is
/// answered independently of the stored verdict.
#[tokio::test]
async fn reads_stale_stored_fail_but_live_satisfied_flips_to_satisfied() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "the violating line\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    // Stored verdict is FAIL, calibrated as `decider = jev` (Jev's noul at
    // plan time agreed with Fail: low noul).
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            false,
            Some("violation found"),
            Some(calibration("jev-1.13.0", 0.05)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    // The violation was fixed since the plan was frozen.
    write_file(dir.path(), "src/a.rs", "the violation was fixed\n");

    let server = MockServer::start().await;
    mock_verify(&server, "jev-1.13.0", 0.95).await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, false, Some("should never run")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Jev);
    assert!(
        outcome.verdict.as_ref().unwrap().success,
        "the live Satisfied answer settles this as satisfied, regardless of the stored FAIL"
    );
    assert!(inner.seen().is_empty());
}

#[tokio::test]
async fn reads_stale_satisfied_but_model_mismatch_decides_agent() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("stale"),
            // Calibrated against jev-1.13.0...
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    write_file(dir.path(), "src/a.rs", "fn a() { /* changed */ }\n");

    let server = MockServer::start().await;
    // ...but the live model resolves to a different version.
    mock_verify(&server, "jev-1.14.0", 0.95).await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("agent decided")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Row: ReadsStale, NotVerified/OverBudget ⇒ Agent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reads_stale_not_verified_decides_agent() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("stale"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    write_file(dir.path(), "src/a.rs", "fn a() { /* changed */ }\n");

    let server = MockServer::start().await;
    mock_verify(&server, "jev-1.13.0", 0.1).await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("agent had the final say")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
    assert_eq!(
        outcome.verdict.as_ref().unwrap().evidence.as_deref(),
        Some("agent had the final say")
    );
}

#[tokio::test]
async fn reads_stale_over_budget_decides_agent() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "small\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("stale"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    // Now huge: the live request's estimated size exceeds Jev's 32k-token
    // budget, so `verify::verify` returns `OverBudget` without any HTTP
    // call at all.
    write_file(dir.path(), "src/a.rs", &"x".repeat(150_000));

    let server = MockServer::start().await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, None));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "an over-budget estimate skips the HTTP call entirely"
    );
}

// ---------------------------------------------------------------------------
// Row: DiscoveryStale ⇒ Agent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn discovery_stale_decides_agent() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "");
    let call = replay::replay_call(
        ReadOnlyTool::Glob,
        &json!({"pattern": "**/*.rs", "path": "."}),
        dir.path(),
    )
    .await;
    let xxh64 = match call.checksums {
        Some(replay::CallChecksum::Single(x)) => x,
        other => panic!("expected a single checksum, got {other:?}"),
    };
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("stale"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Glob {
                input: json!({"pattern": "**/*.rs", "path": "."}),
                xxh64,
            }],
        ),
    );
    // A new file matching the frozen glob: the discovered file SET changed.
    write_file(dir.path(), "src/new.rs", "");

    let server = MockServer::start().await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("agent re-explored")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "DiscoveryStale never even asks Jev"
    );
}

// ---------------------------------------------------------------------------
// Row: decider = jev but the stored calibration no longer agrees under the
// CONFIGURED (raised) threshold ⇒ Agent, no model call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn raised_threshold_no_longer_agreeing_decides_agent_with_no_model_call() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    // Calibrated at plan time with threshold 0.75 (noul 0.8 agreed).
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("cached"),
            Some(calibration("jev-1.13.0", 0.8)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    // File is UNCHANGED — if the threshold hadn't moved, this would be a
    // zero-cost `Fresh`/`Cached` row. Content-freshness is irrelevant here:
    // this row is decided purely from the plan.

    let server = MockServer::start().await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("agent re-verified")));
    // The operator raised the threshold to 0.95: 0.8 no longer agrees.
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.95, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "no model call — this is computed from the plan alone"
    );
    // The agent path still acquires its sandbox normally.
    assert_eq!(sandbox.sources().len(), 1);
}

// ---------------------------------------------------------------------------
// Retries never redo Jev work
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_retry_goes_straight_to_the_agent_without_consulting_the_plan() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("cached"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );

    let server = MockServer::start().await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("agent")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    // `attempt: 2` — even though the plan entry would otherwise settle this
    // check `Cached` with zero cost, a retry always goes straight to the
    // agent.
    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 2);
    let outcome = exec.run_check(req).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// Error kinds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn jev_transport_error_escalates_to_the_agent() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("stale"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    write_file(dir.path(), "src/a.rs", "fn a() { /* changed */ }\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("agent decided")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
}

#[tokio::test]
async fn jev_exhausted_error_escalates_to_the_agent() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("stale"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    write_file(dir.path(), "src/a.rs", "fn a() { /* changed */ }\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(529))
        .mount(&server)
        .await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("agent decided")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);
}

#[tokio::test]
async fn jev_unauthorized_aborts_with_zero_agent_runs() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("stale"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    write_file(dir.path(), "src/a.rs", "fn a() { /* changed */ }\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("must never run")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let err = with_api_key(|| exec.run_check(req)).await.unwrap_err();

    assert!(err.downcast_ref::<AbortCheckRun>().is_some());
    assert!(inner.seen().is_empty(), "zero agent runs on an abort");
}

#[tokio::test]
async fn jev_missing_api_key_aborts_with_zero_agent_runs() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("stale"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );
    write_file(dir.path(), "src/a.rs", "fn a() { /* changed */ }\n");

    let server = MockServer::start().await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, Some("must never run")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    // No `with_api_key` guard: `TYPESAFE_API_KEY` is whatever this process's
    // environment happens to have — serialize against the other tests in
    // this file that mutate it, and force it unset for this call.
    let _guard = API_KEY_ENV_LOCK.lock().await;
    let previous = std::env::var(TYPESAFE_API_KEY_VAR).ok();
    // SAFETY: serialized by `API_KEY_ENV_LOCK`.
    unsafe {
        std::env::remove_var(TYPESAFE_API_KEY_VAR);
    }
    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let err = exec.run_check(req).await.unwrap_err();
    unsafe {
        if let Some(value) = previous {
            std::env::set_var(TYPESAFE_API_KEY_VAR, value);
        }
    }

    assert!(err.downcast_ref::<AbortCheckRun>().is_some());
    assert!(inner.seen().is_empty());
}

// ---------------------------------------------------------------------------
// Plan-load caching (code review on MULTI-1825): loaded/parsed, and a
// corrupt file warned about, at most ONCE per directory — not once per
// check.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn corrupt_plan_is_loaded_and_warned_about_at_most_once_across_two_checks() {
    let dir = TempDir::new().unwrap();
    // An unknown schema version: `PlanStore::load` fails with
    // `PlanError::UnknownVersion` every time it's actually read.
    write_file(dir.path(), ".check-plan.toml", "version = 999999\n");

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("agent"))
            .with_report(1, true, Some("agent")),
    );
    let exec = executor(
        inner.clone(),
        JevClient::new("https://unused.invalid").unwrap(),
        jev_config(0.75, "https://unused.invalid"),
        false,
    );

    let counter = WarnCounter::default();
    let (first, second) = {
        let _guard = tracing::subscriber::set_default(counter.clone());
        let mut req0 = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
        req0.check_id = 0;
        let first = exec.run_check(req0).await.unwrap();

        // A second check under the SAME directory must reuse the first
        // load's (failed) result, not re-parse and re-warn.
        let mut req1 = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
        req1.check_id = 1;
        let second = exec.run_check(req1).await.unwrap();
        (first, second)
    };

    assert_eq!(first.decided_by, DecidedBy::Agent);
    assert_eq!(second.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0, 1]);
    assert_eq!(
        counter.count(),
        1,
        "a corrupt plan is parsed and warned about at most once per directory"
    );
}

#[tokio::test]
async fn good_plan_is_parsed_once_and_reused_even_after_the_file_is_removed() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "src/a.rs", "fn a() {}\n");
    let xxh64 = read_checksum(dir.path(), "src/a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("frozen evidence explanation"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "src/a.rs"}),
                xxh64,
            }],
        ),
    );

    let server = MockServer::start().await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, true, None));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let mut req0 = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
    req0.check_id = 0;
    let first = exec.run_check(req0).await.unwrap();
    assert_eq!(first.decided_by, DecidedBy::Cached);

    // Remove the plan file entirely. If the second check below re-read
    // from disk, `PlanStore::load` would see no file at all and decide
    // `Agent` — so `second` settling `Cached` again can only mean this
    // executor reused the FIRST call's already-parsed `PlanFile`, never
    // touching the filesystem a second time.
    std::fs::remove_file(dir.path().join(".check-plan.toml")).unwrap();

    let mut req1 = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    req1.check_id = 0;
    let second = exec.run_check(req1).await.unwrap();

    assert_eq!(
        second.decided_by,
        DecidedBy::Cached,
        "the parsed plan was reused from the in-memory cache, not re-read from disk"
    );
    assert!(inner.seen().is_empty(), "the agent never ran");
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_plan_is_parsed_and_warned_about_at_most_once_even_when_checks_race() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), ".check-plan.toml", "version = 999999\n");

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("agent"))
            .with_report(1, true, Some("agent")),
    );
    let exec = Arc::new(executor(
        inner.clone(),
        JevClient::new("https://unused.invalid").unwrap(),
        jev_config(0.75, "https://unused.invalid"),
        false,
    ));

    let mut req0 = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
    req0.check_id = 0;
    let mut req1 = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    req1.check_id = 1;

    // A GLOBAL (not thread-local) default: the two checks below are
    // dispatched onto separate worker threads via `tokio::spawn` — the
    // whole point of this test is real OS-thread contention on
    // `JevExecutor::plan_cache` — and `tracing::subscriber::set_default`'s
    // thread-local scoping (used by every other warn-counting test in this
    // file) would silently miss whichever check lands on the thread that
    // didn't install it. Safe to install a global default exactly once
    // here: this repo's canonical test runner, `cargo nextest run` (see
    // `CLAUDE.md`), isolates every test into its own process, so no other
    // test's subscriber can conflict with this one-time global default.
    let counter = WarnCounter::default();
    tracing::subscriber::set_global_default(counter.clone())
        .expect("no other global default is set under nextest's per-test process isolation");

    let exec0 = exec.clone();
    let exec1 = exec.clone();
    let (r0, r1) = tokio::join!(
        tokio::spawn(async move { exec0.run_check(req0).await }),
        tokio::spawn(async move { exec1.run_check(req1).await }),
    );

    let first = r0.unwrap().unwrap();
    let second = r1.unwrap().unwrap();
    assert_eq!(first.decided_by, DecidedBy::Agent);
    assert_eq!(second.decided_by, DecidedBy::Agent);
    assert_eq!(
        counter.count(),
        1,
        "a corrupt plan is parsed and warned about at most once even when two checks race on it"
    );
}

// ---------------------------------------------------------------------------
// `build_evidence`: excludes missing/truncated, fills `windowed`
// ---------------------------------------------------------------------------

#[test]
fn build_evidence_excludes_missing_and_truncated_and_fills_windowed() {
    let missing = replay::ReplayedCall {
        tool: ReadOnlyTool::Read,
        input: json!({"file_path": "gone.rs"}),
        output: None,
        checksums: None,
        windowed: false,
        truncated: false,
        missing: true,
    };
    let truncated = replay::ReplayedCall {
        tool: ReadOnlyTool::Grep,
        input: json!({"pattern": "x", "path": "."}),
        output: Some("capped".to_string()),
        checksums: None,
        windowed: false,
        truncated: true,
        missing: false,
    };
    let kept = replay::ReplayedCall {
        tool: ReadOnlyTool::Read,
        input: json!({"file_path": "a.rs", "offset": 0, "limit": 5}),
        output: Some("partial content".to_string()),
        checksums: Some(replay::CallChecksum::Single("abc".to_string())),
        windowed: true,
        truncated: false,
        missing: false,
    };

    let evidence = build_evidence(&[missing, truncated, kept]);
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].output, "partial content");
    assert!(evidence[0].windowed);
}

// ---------------------------------------------------------------------------
// Self-healing (MULTI-1826)
// ---------------------------------------------------------------------------

/// The motivating scenario from the ticket: a sanitization call moves from
/// `a.rs` to `b.rs`. The frozen plan still points at `a.rs`, whose content
/// changed (the call moved away) — `ReadsStale`, live Jev `NotVerified`
/// (possible false positive) ⇒ agent. The agent explores the real tree and
/// finds the call in `b.rs`, reports satisfied. Self-healing must REPLACE
/// the entry with `entry_from_outcome` of that fresh trace alone: it
/// contains the `Read` of `b.rs` and — since plans must not grow
/// monotonically — no call absent from the new trace, i.e. the stale `Read`
/// of `a.rs` is gone. The next run then settles `Cached` with zero agent
/// runs.
#[tokio::test]
async fn false_positive_heals_by_replacing_the_entry_with_the_agents_fresh_trace() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "a.rs", "AAA_ORIGINAL sanitize call\n");
    let stale_xxh64 = read_checksum(dir.path(), "a.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            true,
            Some("frozen: the sanitize call lives in a.rs"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "a.rs"}),
                xxh64: stale_xxh64,
            }],
        ),
    );
    // The call moved out of a.rs...
    write_file(dir.path(), "a.rs", "AAA_EDITED no call here\n");
    // ...into a file the frozen plan never reads.
    write_file(dir.path(), "b.rs", "BBB_MOVED sanitize call\n");

    let server = MockServer::start().await;
    // The live `ReadsStale` consult sees only a.rs's new (call-free)
    // content and correctly answers NotVerified — a possible false
    // positive, so this escalates to the agent.
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_string_contains("AAA_EDITED"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {"satisfied": {"type": "noul", "noul": 0.1}},
            "usage": {"input_tokens": 20, "output_tokens": 5},
        })))
        .mount(&server)
        .await;
    // Healing's calibration main call sees the agent's own fresh trace
    // (b.rs) and agrees the check passes.
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_string_contains("BBB_MOVED"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {"satisfied": {"type": "noul", "noul": 0.95}},
            "usage": {"input_tokens": 20, "output_tokens": 5},
        })))
        .mount(&server)
        .await;
    // Healing's negative control (empty evidence) correctly rejects.
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_string_contains("\"evidence\":[]"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {"satisfied": {"type": "noul", "noul": 0.02}},
            "usage": {"input_tokens": 10, "output_tokens": 5},
        })))
        .mount(&server)
        .await;

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("found the sanitize call in b.rs"))
            .with_tool_calls(
                0,
                vec![ToolCall {
                    tool: ReadOnlyTool::Read,
                    input: json!({"file_path": dir.path().join("b.rs").to_string_lossy()}),
                }],
            ),
    );
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
    // `run_check` and `finalize` share ONE `with_api_key` critical section:
    // the healing task `run_check` spawns is detached (MULTI-1826 — it must
    // never block settlement) and only `finalize` is guaranteed to await it
    // to completion, so `TYPESAFE_API_KEY` must stay set for the whole span
    // in between, not just around `run_check` itself.
    let outcome = with_api_key(|| async {
        let outcome = exec.run_check(req).await.unwrap();
        exec.finalize().await;
        outcome
    })
    .await;
    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert!(outcome.verdict.as_ref().unwrap().success);
    assert_eq!(inner.seen(), vec![0]);

    let healed = PlanStore::load(dir.path())
        .unwrap()
        .expect("plan still exists");
    let entry = healed
        .lookup(
            "CHECKS.md",
            0,
            0,
            &plan_file::prompt_xxh64(&check().title, &check().prompt),
        )
        .expect("healed entry present");
    assert_eq!(entry.decider, Decider::Jev);
    assert!(entry.verdict);
    assert_eq!(
        entry.calls.len(),
        1,
        "the stale Read of a.rs must be gone, not merged with the new one: {:?}",
        entry.calls
    );
    match &entry.calls[0] {
        PlanCall::Read { input, .. } => {
            assert_eq!(
                input.get("file_path").and_then(|v| v.as_str()),
                Some("b.rs"),
                "the healed entry must contain the Read of b.rs, not a.rs"
            );
        }
        other => panic!("expected a Read call, got {other:?}"),
    }

    // Next run on the unchanged (post-heal) tree: `Fresh` ⇒ `Cached`, zero
    // agent runs.
    let inner2 = Arc::new(FakeExecutor::new());
    let exec2 = executor(
        inner2.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );
    let req2 = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome2 = exec2.run_check(req2).await.unwrap();
    assert_eq!(outcome2.decided_by, DecidedBy::Cached);
    assert!(outcome2.verdict.as_ref().unwrap().success);
    assert!(
        inner2.seen().is_empty(),
        "the healed entry must settle the next run with zero agent runs"
    );
}

/// The confirmed-failure counterpart: the agent's own verdict is a genuine
/// FAIL, which is a valid entry to freeze — the next run on an unchanged
/// tree must report the cached failure, not re-run the agent.
#[tokio::test]
async fn confirmed_failure_heals_to_a_cached_failure_on_the_next_run() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "c.rs", "CCC_VIOLATION content\n");

    let server = MockServer::start().await;
    // No existing plan: `decide()` finds no entry at all and escalates
    // immediately — no live Jev call at decide time.
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_string_contains("\"evidence\":[]"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {"satisfied": {"type": "noul", "noul": 0.02}},
            "usage": {"input_tokens": 10, "output_tokens": 5},
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_string_contains("CCC_VIOLATION"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {"satisfied": {"type": "noul", "noul": 0.05}},
            "usage": {"input_tokens": 20, "output_tokens": 5},
        })))
        .mount(&server)
        .await;

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, false, Some("a real violation"))
            .with_tool_calls(
                0,
                vec![ToolCall {
                    tool: ReadOnlyTool::Read,
                    input: json!({"file_path": dir.path().join("c.rs").to_string_lossy()}),
                }],
            ),
    );
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
    // See `false_positive_heals_by_replacing_the_entry_with_the_agents_fresh_trace`'s
    // comment on why `run_check` and `finalize` share one `with_api_key`
    // critical section.
    let outcome = with_api_key(|| async {
        let outcome = exec.run_check(req).await.unwrap();
        exec.finalize().await;
        outcome
    })
    .await;
    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert!(!outcome.verdict.as_ref().unwrap().success);

    let healed = PlanStore::load(dir.path())
        .unwrap()
        .expect("healing creates a plan from scratch when none existed");
    let entry = healed
        .lookup(
            "CHECKS.md",
            0,
            0,
            &plan_file::prompt_xxh64(&check().title, &check().prompt),
        )
        .expect("healed entry present");
    assert_eq!(entry.decider, Decider::Jev);
    assert!(!entry.verdict);

    let inner2 = Arc::new(FakeExecutor::new());
    let exec2 = executor(
        inner2.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );
    let req2 = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome2 = exec2.run_check(req2).await.unwrap();
    assert_eq!(outcome2.decided_by, DecidedBy::Cached);
    assert!(!outcome2.verdict.as_ref().unwrap().success);
    assert!(inner2.seen().is_empty());
}

/// `ReadsStale` + live Jev `Satisfied` refreshes the stored entry's
/// checksums, verdict, and calibrated `noul` in place — but keeps
/// `control_noul` (the negative control depends only on the prompt and
/// model, so it is never re-run for this path). The next run then settles
/// `Cached` with zero agent AND zero Jev calls.
#[tokio::test]
async fn reads_stale_satisfied_refreshes_in_place_and_next_run_is_fully_cached() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "g.rs", "the violating line\n");
    let stale_xxh64 = read_checksum(dir.path(), "g.rs").await;
    write_plan(
        dir.path(),
        plan_entry(
            &check(),
            Decider::Jev,
            false,
            Some("violation found"),
            Some(calibration("jev-1.13.0", 0.05)),
            vec![PlanCall::Read {
                input: json!({"file_path": "g.rs"}),
                xxh64: stale_xxh64,
            }],
        ),
    );
    // The violation was fixed since the plan was frozen.
    write_file(dir.path(), "g.rs", "the violation was fixed\n");
    let fresh_xxh64 = read_checksum(dir.path(), "g.rs").await;

    let server = MockServer::start().await;
    mock_verify(&server, "jev-1.13.0", 0.95).await;

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(FakeExecutor::new().with_report(0, false, Some("must never run")));
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox.clone(), plan_identity(dir.path()), 1);
    let outcome = with_api_key(|| exec.run_check(req)).await.unwrap();
    assert_eq!(outcome.decided_by, DecidedBy::Jev);
    assert!(outcome.verdict.as_ref().unwrap().success);
    assert!(inner.seen().is_empty());

    exec.finalize().await;

    let healed = PlanStore::load(dir.path()).unwrap().unwrap();
    let entry = healed
        .lookup(
            "CHECKS.md",
            0,
            0,
            &plan_file::prompt_xxh64(&check().title, &check().prompt),
        )
        .unwrap();
    assert!(entry.verdict, "verdict refreshed to satisfied");
    let cal = entry.jev.as_ref().expect("calibration kept");
    assert_eq!(cal.noul, 0.95, "noul refreshed to the live call's");
    assert_eq!(
        cal.control_noul, 0.02,
        "control_noul kept from plan time — the control is never re-run here"
    );
    match &entry.calls[0] {
        PlanCall::Read { xxh64, .. } => {
            assert_eq!(
                xxh64, &fresh_xxh64,
                "checksum refreshed to the fixed content"
            )
        }
        other => panic!("expected a Read call, got {other:?}"),
    }

    // Next run: `Fresh` (checksums now match) ⇒ `Cached`, zero agent AND
    // zero Jev calls.
    let requests_before = server.received_requests().await.unwrap().len();
    let inner2 = Arc::new(FakeExecutor::new());
    let exec2 = executor(
        inner2.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );
    let req2 = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome2 = exec2.run_check(req2).await.unwrap();
    assert_eq!(outcome2.decided_by, DecidedBy::Cached);
    assert!(inner2.seen().is_empty());
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        requests_before,
        "zero new Jev calls on the fully-fresh next run"
    );
}

/// `--frozen`: every plan file stays byte-identical and zero calibration
/// calls are made, even though a check escalates to the agent.
#[tokio::test]
async fn frozen_flag_never_writes_and_makes_zero_calibration_calls() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "d.rs", "DDD content\n");
    let xxh64 = read_checksum(dir.path(), "d.rs").await;
    // An existing, unrelated entry so "byte-identical" is a meaningful
    // assertion, not just "no file appeared".
    write_plan(
        dir.path(),
        plan_entry(
            &Check {
                title: "Another check".to_string(),
                prompt: "unrelated".to_string(),
            },
            Decider::Jev,
            true,
            Some("unrelated cached evidence"),
            Some(calibration("jev-1.13.0", 0.9)),
            vec![PlanCall::Read {
                input: json!({"file_path": "d.rs"}),
                xxh64,
            }],
        ),
    );
    let before = std::fs::read(dir.path().join(".check-plan.toml")).unwrap();

    // No mock mounted at all: any HTTP request would either fail the
    // assertion below via a nonzero received-request count, or (for an
    // unmatched request) 404 — either way proving a call was attempted.
    let server = MockServer::start().await;
    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("agent decided"))
            .with_tool_calls(
                0,
                vec![ToolCall {
                    tool: ReadOnlyTool::Read,
                    input: json!({"file_path": dir.path().join("d.rs").to_string_lossy()}),
                }],
            ),
    );
    let exec = frozen_executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
    );

    // `check()` ("No yellow") has no entry in the plan above ("Another
    // check" is a different check) — escalates immediately, no live Jev
    // call at decide time either.
    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    let outcome = exec.run_check(req).await.unwrap();
    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(inner.seen(), vec![0]);

    exec.finalize().await;

    let after = std::fs::read(dir.path().join(".check-plan.toml")).unwrap();
    assert_eq!(
        before, after,
        "--frozen: the plan file must be byte-identical"
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "--frozen: zero calibration calls",
    );
}

/// The check's outcome is returned before any calibration call is made — and
/// deterministically so, not merely "usually": since `queue_agent_escalation`
/// only ever records the escalation (no I/O, nothing spawned —
/// `healer::Healer`'s module docs), zero requests can possibly have reached
/// Jev at the moment `run_check` returns, full stop, on any runtime flavor.
/// The first calibration request only appears once `finalize` explicitly
/// builds the queued entry.
#[tokio::test]
async fn healed_outcome_is_returned_before_the_calibration_call_completes() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "e.rs", "EEE content\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {"satisfied": {"type": "noul", "noul": 0.9}},
            "usage": {"input_tokens": 20, "output_tokens": 5},
        })))
        .mount(&server)
        .await;

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("agent decided"))
            .with_tool_calls(
                0,
                vec![ToolCall {
                    tool: ReadOnlyTool::Read,
                    input: json!({"file_path": dir.path().join("e.rs").to_string_lossy()}),
                }],
            ),
    );
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);

    // No plan exists, so the live decide-time path makes no Jev call either
    // — the only calibration traffic in this whole test is healing's own,
    // which must not appear until `finalize` runs.
    let outcome = exec.run_check(req).await.unwrap();
    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "no calibration call may have reached Jev before run_check returned",
    );

    with_api_key(|| exec.finalize()).await;

    // Both the main and negative-control calibration calls.
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

/// A failure while healing (here, the calibration call 500s) is logged and
/// skipped: it changes neither this run's already-returned verdict nor the
/// previous plan entry, which is left exactly as it was.
#[tokio::test]
async fn calibration_failure_during_healing_leaves_the_verdict_and_previous_entry_intact() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "f.rs", "fn f() {}\n");
    let xxh64 = read_checksum(dir.path(), "f.rs").await;
    // A prompt-hash mismatch (mirroring `prompt_hash_mismatch_decides_agent`)
    // forces an immediate escalation with zero decide-time Jev calls, so the
    // only Jev traffic in this test is healing's own (failing) calibration.
    let mut entry = plan_entry(
        &check(),
        Decider::Jev,
        true,
        Some("previously cached"),
        Some(calibration("jev-1.13.0", 0.9)),
        vec![PlanCall::Read {
            input: json!({"file_path": "f.rs"}),
            xxh64,
        }],
    );
    entry.prompt_xxh64 = plan_file::prompt_xxh64("a different title", "a different prompt");
    write_plan(dir.path(), entry);
    let before = std::fs::read(dir.path().join(".check-plan.toml")).unwrap();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("agent decided, unaffected by healing"))
            .with_tool_calls(
                0,
                vec![ToolCall {
                    tool: ReadOnlyTool::Read,
                    input: json!({"file_path": dir.path().join("f.rs").to_string_lossy()}),
                }],
            ),
    );
    let exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    // See `false_positive_heals_by_replacing_the_entry_with_the_agents_fresh_trace`'s
    // comment on why `run_check` and `finalize` share one `with_api_key`
    // critical section.
    let outcome = with_api_key(|| async {
        let outcome = exec.run_check(req).await.unwrap();
        exec.finalize().await;
        outcome
    })
    .await;
    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert!(
        outcome.verdict.as_ref().unwrap().success,
        "this run's verdict must be unaffected by a later healing failure"
    );
    assert_eq!(
        outcome.verdict.as_ref().unwrap().evidence.as_deref(),
        Some("agent decided, unaffected by healing")
    );

    let after = std::fs::read(dir.path().join(".check-plan.toml")).unwrap();
    assert_eq!(
        before, after,
        "a calibration failure during healing must leave the previous entry untouched"
    );
}

/// A heal that never completes in time (an artificially slow Jev response)
/// is abandoned once it exceeds its timeout (code review): `finalize` still
/// returns, this run's already-reported verdict is unaffected, and the
/// previous plan entry is left exactly as it was. Uses
/// `Healer::set_heal_timeout_for_test` to shrink the timeout so the test
/// doesn't have to wait the real, generous production duration.
#[tokio::test]
async fn a_heal_that_exceeds_its_timeout_is_abandoned_and_leaves_the_previous_entry_intact() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "h.rs", "fn h() {}\n");
    let xxh64 = read_checksum(dir.path(), "h.rs").await;
    // A prompt-hash mismatch (mirroring `prompt_hash_mismatch_decides_agent`)
    // forces an immediate escalation with zero decide-time Jev calls.
    let mut entry = plan_entry(
        &check(),
        Decider::Jev,
        true,
        Some("previously cached"),
        Some(calibration("jev-1.13.0", 0.9)),
        vec![PlanCall::Read {
            input: json!({"file_path": "h.rs"}),
            xxh64,
        }],
    );
    entry.prompt_xxh64 = plan_file::prompt_xxh64("a different title", "a different prompt");
    write_plan(dir.path(), entry);
    let before = std::fs::read(dir.path().join(".check-plan.toml")).unwrap();

    let server = MockServer::start().await;
    // Far longer than the shrunk test timeout below. Never actually waited
    // out: `tokio::time::timeout` races it, it doesn't sleep for it.
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "model": "jev-1.13.0",
                    "answers": {"satisfied": {"type": "noul", "noul": 0.9}},
                    "usage": {"input_tokens": 20, "output_tokens": 5},
                }))
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&server)
        .await;

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("agent decided, unaffected by a stuck heal"))
            .with_tool_calls(
                0,
                vec![ToolCall {
                    tool: ReadOnlyTool::Read,
                    input: json!({"file_path": dir.path().join("h.rs").to_string_lossy()}),
                }],
            ),
    );
    let mut exec = executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    );
    exec.healer
        .set_heal_timeout_for_test(Duration::from_millis(50));

    let req = request(dir.path(), sandbox, plan_identity(dir.path()), 1);
    // See `false_positive_heals_by_replacing_the_entry_with_the_agents_fresh_trace`'s
    // comment on why `run_check` and `finalize` share one `with_api_key`
    // critical section.
    let outcome = with_api_key(|| async {
        let outcome = exec.run_check(req).await.unwrap();
        exec.finalize().await;
        outcome
    })
    .await;

    assert_eq!(outcome.decided_by, DecidedBy::Agent);
    assert!(
        outcome.verdict.as_ref().unwrap().success,
        "this run's verdict must be unaffected by a heal that later times out"
    );

    let after = std::fs::read(dir.path().join(".check-plan.toml")).unwrap();
    assert_eq!(
        before, after,
        "a heal that exceeds its timeout must leave the previous entry untouched"
    );
}

// ---------------------------------------------------------------------------
// `crate::checks::finalize_if_settled` (code review, MULTI-1826): an aborted
// run must skip `finalize` entirely, even when a heal was already queued.
// ---------------------------------------------------------------------------

/// Two checks share one directory (and so one `.check-plan.toml`): check 0
/// has no existing entry, so it escalates immediately, its agent passes, and
/// `run_agent` queues a heal for it — synchronously, making zero Jev calls
/// (see `healer`'s docs). Check 1 has a `ReadsStale` entry whose live
/// consult hits the mounted `401`, aborting the whole run.
/// `crate::checks::finalize_if_settled` — the exact sequencing `multi
/// check`'s real entrypoint uses — must then skip `finalize` altogether:
/// zero calibration calls (check 0's queued heal is simply dropped,
/// un-built), and the plan file stays byte-identical.
#[tokio::test]
async fn finalize_if_settled_skips_finalize_on_abort_even_with_a_pending_heal() {
    let dir = TempDir::new().unwrap();
    write_file(dir.path(), "i.rs", "fn i() {}\n");
    let xxh64 = read_checksum(dir.path(), "i.rs").await;

    let check0 = check();
    let check1 = Check {
        title: "Check B".to_string(),
        prompt: "prompt b".to_string(),
    };

    // Only check 1 has a stored entry (ordinal 1) — check 0 (ordinal 0) has
    // none, so it escalates trivially, with no live decide-time Jev call.
    let plan = PlanFile::new(vec![PlanRequirement {
        title: "R".to_string(),
        source: "CHECKS.md".to_string(),
        ordinal: 0,
        checks: vec![PlanCheck {
            title: check1.title.clone(),
            ordinal: 1,
            prompt_xxh64: plan_file::prompt_xxh64(&check1.title, &check1.prompt),
            decider: Decider::Jev,
            verdict: true,
            evidence: Some("stale".to_string()),
            jev: Some(calibration("jev-1.13.0", 0.9)),
            calls: vec![PlanCall::Read {
                input: json!({"file_path": "i.rs"}),
                xxh64,
            }],
        }],
    }]);
    PlanStore::write(dir.path(), &plan).unwrap();
    let before = std::fs::read(dir.path().join(".check-plan.toml")).unwrap();
    // Edited after freezing: check 1's replay is `ReadsStale`, so it
    // consults Jev live — and that call 401s.
    write_file(dir.path(), "i.rs", "fn i() { /* changed */ }\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let sandbox = Arc::new(RecordingSandbox::new());
    let inner = Arc::new(
        FakeExecutor::new()
            .with_report(0, true, Some("queued a heal before the run aborted"))
            .with_tool_calls(
                0,
                vec![ToolCall {
                    tool: ReadOnlyTool::Read,
                    input: json!({"file_path": dir.path().join("i.rs").to_string_lossy()}),
                }],
            ),
    );
    let exec: Arc<dyn crate::checks::executor::CheckExecutor + Send + Sync> = Arc::new(executor(
        inner.clone(),
        JevClient::new(server.uri()).unwrap(),
        jev_config(0.75, &server.uri()),
        false,
    ));

    let identity0 = plan_identity(dir.path()); // check_ordinal: 0
    let identity1 = PlanIdentity {
        check_ordinal: 1,
        ..plan_identity(dir.path())
    };

    with_api_key(|| async {
        let req0 = AgentRunRequest {
            check_id: 0,
            check: check0,
            source_dir: dir.path().to_path_buf(),
            sandbox: crate::checks::sandbox::SandboxLease::new(
                sandbox.clone(),
                dir.path().to_path_buf(),
            ),
            declared_in: PathBuf::from("CHECKS.md"),
            attempt: 1,
            progress: None,
            plan: identity0,
        };
        let outcome0 = exec.run_check(req0).await.unwrap();
        assert_eq!(outcome0.decided_by, DecidedBy::Agent);
        assert_eq!(
            inner.seen(),
            vec![0],
            "check 0's agent ran and queued a heal"
        );
        // Nothing built yet — queuing is synchronous and does no I/O.
        assert_eq!(server.received_requests().await.unwrap().len(), 0);

        let req1 = AgentRunRequest {
            check_id: 1,
            check: check1,
            source_dir: dir.path().to_path_buf(),
            sandbox: crate::checks::sandbox::SandboxLease::new(sandbox, dir.path().to_path_buf()),
            declared_in: PathBuf::from("CHECKS.md"),
            attempt: 1,
            progress: None,
            plan: identity1,
        };
        let err = exec.run_check(req1).await.unwrap_err();
        assert!(err.downcast_ref::<AbortCheckRun>().is_some());
        // Exactly one request so far: check 1's own decide-time consult,
        // the one that produced the 401 the abort is built from.
        let requests_at_abort = server.received_requests().await.unwrap().len();
        assert_eq!(requests_at_abort, 1);

        // The exact sequencing `crate::checks::run` uses: an `Err` pipeline
        // result must skip `finalize` entirely.
        let result = crate::checks::finalize_if_settled(&exec, Err(err)).await;
        assert!(result.is_err(), "the abort must still propagate");
    })
    .await;

    // Check 0's queued heal was never built: `finalize` never ran, so it
    // made zero FURTHER calibration calls beyond the one that caused the
    // abort itself...
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "an aborted run must make zero further Jev calls after the abort, \
         including for an already-queued heal from a check that settled \
         before it",
    );
    // ...and the plan file is untouched.
    let after = std::fs::read(dir.path().join(".check-plan.toml")).unwrap();
    assert_eq!(
        before, after,
        "an aborted run must write nothing, even with a pending heal queued",
    );
}
