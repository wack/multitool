//! A test-only [`Planner`] double: returns scripted [`PlannedCheck`]s (or a
//! scripted error) per check id, without running any agent or talking to
//! Jev. Mirrors [`crate::checks::executor::FakeExecutor`]'s shape.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use miette::{Report, Result, miette};

use super::{AbortPlanRun, PlanRequest, PlannedCheck, Planner};
use crate::checks::jev::error::JevError;
use crate::checks::model::CheckId;

#[derive(Default)]
pub struct FakePlanner {
    scripted: HashMap<CheckId, PlannedCheck>,
    errors: HashMap<CheckId, String>,
    /// Check ids scripted to fail with an [`AbortPlanRun`]-wrapped error —
    /// distinct from `errors`, whose failures are plain per-check errors.
    aborts: HashMap<CheckId, String>,
    /// Every `check_id` the fake was asked to plan, in call order.
    seen: Mutex<Vec<CheckId>>,
}

impl FakePlanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Script `plan_check(id)` to succeed with `planned`.
    pub fn with_planned(mut self, id: CheckId, planned: PlannedCheck) -> Self {
        self.scripted.insert(id, planned);
        self
    }

    /// Script `plan_check(id)` to fail with a plain per-check error carrying
    /// `message`.
    pub fn with_error(mut self, id: CheckId, message: impl Into<String>) -> Self {
        self.errors.insert(id, message.into());
        self
    }

    /// Script `plan_check(id)` to fail with an [`AbortPlanRun`]-wrapped
    /// error — the orchestrator (`crate::checks::plan::run_with_planner`)
    /// detects this via `downcast_ref` and aborts the whole run rather than
    /// reporting just this check as `error`.
    pub fn with_abort(mut self, id: CheckId, message: impl Into<String>) -> Self {
        self.aborts.insert(id, message.into());
        self
    }

    /// The check ids the fake was asked to plan, in call order — lets tests
    /// assert e.g. "the planner was invoked zero times" on a fully-reused
    /// run, or "invoked exactly once" for the one affected check on a
    /// partial re-plan.
    pub fn seen(&self) -> Vec<CheckId> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl Planner for FakePlanner {
    async fn plan_check(&self, req: PlanRequest) -> Result<PlannedCheck> {
        self.seen.lock().unwrap().push(req.check_id);
        if let Some(message) = self.aborts.get(&req.check_id) {
            return Err(Report::new(AbortPlanRun(JevError::Transport(
                message.clone(),
            ))));
        }
        if let Some(message) = self.errors.get(&req.check_id) {
            return Err(miette!("{message}"));
        }
        self.scripted
            .get(&req.check_id)
            .cloned()
            .ok_or_else(|| miette!("FakePlanner: no script for check {}", req.check_id))
    }
}
