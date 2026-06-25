//! A test-only [`CheckExecutor`] double: returns scripted verdicts per check id
//! without spawning a process, building a model, or touching the network, so the
//! execution → reconciliation → reporting pipeline can be driven
//! deterministically. (Tests & docs, MULTI-1354; updated for MULTI-1367.)

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use async_trait::async_trait;
use miette::Result;

use super::judge::CheckReport;
use super::{AgentOutcome, AgentRunRequest, CheckExecutor};
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

    /// The check ids the fake was asked to run, in call order.
    pub fn seen(&self) -> Vec<CheckId> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl CheckExecutor for FakeExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        let attempt = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(req.check_id);
            seen.iter().filter(|id| **id == req.check_id).count()
        };

        if self.silent.contains(&req.check_id) {
            return Ok(AgentOutcome {
                verdict: None,
                stop_reason: Some("fake: finished without reporting".into()),
                turns: 1,
                error: None,
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
            });
        }

        Ok(AgentOutcome {
            verdict: self.scripted.get(&req.check_id).cloned(),
            stop_reason: Some("fake: reported".into()),
            turns: 1,
            error: None,
        })
    }
}
