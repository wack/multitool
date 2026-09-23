//! A test-only [`CheckExecutor`] double: returns scripted verdicts per check id
//! without spawning a process, building a model, or touching the network, so the
//! execution → reconciliation → reporting pipeline can be driven
//! deterministically. (Tests & docs, MULTI-1354; updated for MULTI-1367.)

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use miette::Result;

use super::judge::CheckReport;
use super::tool_capture::ToolCall;
use super::{AgentOutcome, AgentRunRequest, CheckExecutor, PlanIdentity};
use crate::checks::model::CheckId;

#[derive(Default)]
pub struct FakeExecutor {
    scripted: HashMap<CheckId, CheckReport>,
    /// Check ids that should simulate an agent finishing without reporting.
    silent: HashSet<CheckId>,
    /// Check ids that stay silent until the Nth attempt, then report. Keyed by
    /// id to `(report_on_attempt, report)`. Exercises the retry path: the same
    /// `CheckId` is re-run, so the fake counts attempts per id.
    silent_until: HashMap<CheckId, (usize, CheckReport)>,
    /// `AgentOutcome::tool_calls` to attach for a given check id, scripted via
    /// `with_tool_calls` (MULTI-1817). Absent ids default to no calls.
    tool_calls: HashMap<CheckId, Vec<ToolCall>>,
    /// Every `(check_id, attempt)` the fake was asked to run, in call order.
    seen: Mutex<Vec<(CheckId, u32)>>,
    /// Every `declared_in` (MULTI-1834) the fake was asked to run with, in
    /// call order — lets tests assert the root-relative "declared in" path
    /// reaches the executor correctly, end to end.
    declared_ins: Mutex<Vec<PathBuf>>,
}

impl FakeExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Script a verdict for `id`.
    pub fn with_report(mut self, id: CheckId, success: bool, evidence: Option<&str>) -> Self {
        self.scripted.insert(
            id,
            CheckReport {
                success,
                evidence: evidence.map(str::to_string),
            },
        );
        self
    }

    /// Make `id` simulate an agent that finishes without reporting a verdict.
    pub fn with_silent(mut self, id: CheckId) -> Self {
        self.silent.insert(id);
        self
    }

    /// Make `id` stay silent until its `report_on_attempt`-th run (1-based), then
    /// report `success`/`evidence`. Used to drive the retry path deterministically.
    pub fn with_silent_until(
        mut self,
        id: CheckId,
        report_on_attempt: usize,
        success: bool,
        evidence: Option<&str>,
    ) -> Self {
        self.silent_until.insert(
            id,
            (
                report_on_attempt,
                CheckReport {
                    success,
                    evidence: evidence.map(str::to_string),
                },
            ),
        );
        self
    }

    /// Script the `tool_calls` an `AgentOutcome` for `id` carries (MULTI-1817),
    /// e.g. to exercise plan-capture logic downstream without a real agent.
    /// `id`'s outcome carries `calls` verbatim, regardless of how it reports —
    /// scripted, silent, or silent-until.
    pub fn with_tool_calls(mut self, id: CheckId, calls: Vec<ToolCall>) -> Self {
        self.tool_calls.insert(id, calls);
        self
    }

    /// The check ids the fake was asked to run, in call order.
    pub fn seen(&self) -> Vec<CheckId> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(id, _)| *id)
            .collect()
    }

    /// The `(check_id, attempt)` pairs the fake was asked to run, in call
    /// order — lets tests assert the retry plumbing threads attempt numbers
    /// through to the executor.
    pub fn seen_attempts(&self) -> Vec<(CheckId, u32)> {
        self.seen.lock().unwrap().clone()
    }

    /// Every `declared_in` (MULTI-1834) the fake was asked to run with, in
    /// call order.
    pub fn declared_ins(&self) -> Vec<PathBuf> {
        self.declared_ins.lock().unwrap().clone()
    }
}

#[async_trait]
impl CheckExecutor for FakeExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        // Mirror `CerseiExecutor`: this fake always "runs an agent", so it
        // always acquires the sandbox lease (MULTI-1818). This is what keeps
        // the `RecordingSandbox`-based e2e assertions (one clone per attempt,
        // cloning the repository root) meaningful with this fake standing in
        // for the real executor. The acquired path is also carried onto
        // every outcome below (MULTI-1826), mirroring `CerseiExecutor`'s own
        // unconditional `AgentOutcome::sandbox_root` — tests that drive
        // `JevExecutor`'s self-healing over this fake need it to relativize
        // a scripted call the same way the real executor's outcome would.
        let sandbox_root = req.sandbox.acquire().await?.to_path_buf();

        let attempt = {
            let mut seen = self.seen.lock().unwrap();
            seen.push((req.check_id, req.attempt));
            seen.iter().filter(|(id, _)| *id == req.check_id).count()
        };
        self.declared_ins
            .lock()
            .unwrap()
            .push(req.declared_in.clone());

        let tool_calls = self
            .tool_calls
            .get(&req.check_id)
            .cloned()
            .unwrap_or_default();

        if self.silent.contains(&req.check_id) {
            return Ok(AgentOutcome {
                verdict: None,
                stop_reason: Some("fake: finished without reporting".into()),
                turns: 1,
                error: None,
                trace_jsonl: None,
                tool_calls,
                sandbox_root: Some(sandbox_root.clone()),
                ..Default::default()
            });
        }

        // Silent until the configured attempt, then report.
        if let Some((report_on, report)) = self.silent_until.get(&req.check_id) {
            let verdict = (attempt >= *report_on).then(|| report.clone());
            return Ok(AgentOutcome {
                verdict,
                stop_reason: Some("fake: silent-until".into()),
                turns: 1,
                error: None,
                trace_jsonl: None,
                tool_calls,
                sandbox_root: Some(sandbox_root.clone()),
                ..Default::default()
            });
        }

        Ok(AgentOutcome {
            verdict: self.scripted.get(&req.check_id).cloned(),
            stop_reason: Some("fake: reported".into()),
            turns: 1,
            error: None,
            trace_jsonl: None,
            tool_calls,
            sandbox_root: Some(sandbox_root),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::tool_capture::ReadOnlyTool;
    use super::*;

    fn req(check_id: CheckId) -> AgentRunRequest {
        AgentRunRequest {
            check_id,
            check: crate::checks::model::Check::new_prompt("t", "t", "p"),
            source_dir: std::path::PathBuf::from("."),
            sandbox: crate::checks::sandbox::SandboxLease::new(
                std::sync::Arc::new(crate::checks::sandbox::RecordingSandbox::new()),
                std::path::PathBuf::from("."),
            ),
            declared_in: std::path::PathBuf::from("CHECKS.toml"),
            attempt: 1,
            progress: None,
            plan: PlanIdentity {
                dir: std::path::PathBuf::from("."),
                requirement_id: "r".to_string(),
                requirement_title: "R".to_string(),
                root_source: crate::checks::model::RootSource::Manifest,
            },
        }
    }

    #[tokio::test]
    async fn scripted_tool_calls_ride_along_with_a_reported_verdict() {
        let calls = vec![ToolCall {
            tool: ReadOnlyTool::Read,
            input: json!({ "file_path": "src/lib.rs" }),
        }];
        let fake = FakeExecutor::new()
            .with_report(0, true, None)
            .with_tool_calls(0, calls.clone());

        let outcome = fake.run_check(req(0)).await.unwrap();
        assert_eq!(outcome.tool_calls, calls);
    }

    #[tokio::test]
    async fn an_unscripted_check_carries_no_tool_calls() {
        let fake = FakeExecutor::new().with_report(0, true, None);
        let outcome = fake.run_check(req(0)).await.unwrap();
        assert!(outcome.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn scripted_tool_calls_also_ride_along_on_silent_and_silent_until_outcomes() {
        let calls = vec![ToolCall {
            tool: ReadOnlyTool::Grep,
            input: json!({ "pattern": "TODO" }),
        }];
        let fake = FakeExecutor::new()
            .with_silent(0)
            .with_tool_calls(0, calls.clone())
            .with_silent_until(1, 2, true, None)
            .with_tool_calls(1, calls.clone());

        let silent_outcome = fake.run_check(req(0)).await.unwrap();
        assert_eq!(silent_outcome.tool_calls, calls);

        let silent_until_outcome = fake.run_check(req(1)).await.unwrap();
        assert_eq!(silent_until_outcome.tool_calls, calls);
    }
}
