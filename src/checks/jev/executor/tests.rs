//! Table-driven tests for [`super::JevExecutor`] — one case per decision-table
//! row (MULTI-1825's own module docs), plus the error-kind and retry
//! contracts. Drives the real `JevExecutor` over a [`FakeExecutor`] inner, a
//! [`RecordingSandbox`], a `wiremock` Jev server, and `.check-plan.toml`
//! files written via [`PlanStore`] in a tempdir — never a real agent, never a
//! real TypeSafe request.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::checks::config::JevConfig;
use crate::checks::executor::{FakeExecutor, PlanIdentity, ReadOnlyTool};
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
    JevExecutor::new(inner, Arc::new(jev_client), jev_config, no_cache)
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
