//! A test-only [`CheckExecutor`] double: returns scripted inline verdicts per
//! check id without spawning any process or touching the MCP server, so the
//! execution → reconciliation → reporting pipeline can be driven
//! deterministically. (Tests & docs, MULTI-1354)

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use miette::Result;

use super::{AgentOutcome, AgentRunRequest, CheckExecutor};
use crate::checks::mcp::CheckReport;
use crate::checks::model::CheckId;

#[derive(Default)]
pub struct FakeExecutor {
    scripted: HashMap<CheckId, CheckReport>,
    /// Check ids that should simulate an agent crashing without reporting.
    silent: HashSet<CheckId>,
    seen: Mutex<Vec<CheckId>>,
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

    /// Make `id` simulate an agent that crashes/exits without reporting.
    pub fn with_silent(mut self, id: CheckId) -> Self {
        self.silent.insert(id);
        self
    }

    /// The check ids the fake was asked to run, in call order.
    pub fn seen(&self) -> Vec<CheckId> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl CheckExecutor for FakeExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        self.seen.lock().unwrap().push(req.check_id);
        if self.silent.contains(&req.check_id) {
            return Ok(AgentOutcome {
                exited_cleanly: false,
                exit_code: Some(1),
                stderr: "fake: agent crashed".into(),
                reported: None,
            });
        }
        Ok(AgentOutcome {
            exited_cleanly: true,
            exit_code: Some(0),
            stderr: String::new(),
            reported: self.scripted.get(&req.check_id).cloned(),
        })
    }
}
