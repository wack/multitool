//! The `Planner` DI seam (MULTI-1824): the abstraction over "run one check's
//! evidence-discovery agent, then decide how it's frozen into a plan entry."
//! Mirrors [`CheckExecutor`]'s own shape exactly — one trait method, a boxed
//! alias for dynamic dispatch, and a test fake ([`fake::FakePlanner`]) — see
//! that trait's module docs.
//!
//! The real implementation, [`AgentPlanner`], **composes** a
//! [`CheckExecutor`] (the reasoning agent still answers "does this evidence
//! satisfy the check?" — the same question `multi check` asks) rather than
//! replacing it: [`AgentPlanner::plan_check`] runs the agent — with the same
//! retry-until-verdict policy `multi check`'s execution phase uses,
//! replicated here in miniature rather than reused directly (see
//! [`AgentPlanner::run_agent`]'s docs on why) — then hands the outcome to
//! [`entry_from_outcome`], the single function that turns a completed agent
//! run into a [`PlannedCheck`]. `entry_from_outcome` runs no agent itself and
//! is unit-testable directly from a scripted [`AgentOutcome`] (see this
//! module's tests) — `plan_check`'s own entry is required to equal
//! `entry_from_outcome`'s for the same outcome, since `plan_check` literally
//! *is* "run the agent, then call `entry_from_outcome`".
//!
//! ## Calibration
//!
//! [`entry_from_outcome`] skips calibration entirely (zero Jev calls) when it
//! would be pointless: zero captured tool calls
//! ([`AgentReason::NoToolCalls`]), or any captured call that replays as
//! missing or truncated ([`AgentReason::TruncatedDiscovery`] — "the plan
//! cannot guard against new files, so Jev is never trusted with it").
//! Otherwise it asks Jev to reproduce the agent's verdict, and — unless that
//! request alone is already [`JevDecision::OverBudget`] (which the control
//! could never change, since removing evidence only *shrinks* the request) —
//! runs the empty-evidence negative control too: "one extra, near-empty Jev
//! call per planned check" is the common-case cost, exactly two calls.
//!
//! [`decide_from_calibration`] is the pure (no I/O) classifier: it decides
//! [`Decider`] from the two [`JevDecision`]s, using [`verify::agreement`]'s
//! banded comparison for *both* the main verdict and the negative control
//! (the ticket: both parts are "judged with the agreement band"). Precedence
//! among competing reasons — before calibration even runs, `no tool calls` >
//! `truncated discovery`; among the two Jev calls, `over budget` (main call
//! only — decided before a control is ever requested) > `verdict
//! disagreement/uncertain` (from the main call) > `control failed` (only
//! reachable once the main call already agreed) — is total: at most one
//! reason is ever recorded, and `decider = "jev"` only when neither applies.
//!
//! ## Errors
//!
//! A [`JevError`] from either call maps to one of two shapes (see
//! [`jev_error_to_report`]): `Unauthorized`/`MissingApiKey`/`Invalid` must
//! **abort the whole `multi plan` run** — a bad or missing credential must
//! never silently read as every check disagreeing with the agent — wrapped as
//! [`AbortPlanRun`], which `crate::checks::plan::run` detects via
//! `Report::downcast_ref`; `Exhausted`/`Transport` error just the one check
//! (transient/single-request failures, consistent with "a check with no
//! verdict is reported as `error`, not planned").

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use miette::{Diagnostic, Result, miette};
use thiserror::Error;

use crate::checks::config::JevConfig;
use crate::checks::executor::{
    AgentOutcome, AgentRunRequest, CheckExecutor, CheckReport, ReadOnlyTool, ToolCall,
};
use crate::checks::jev::client::JevClient;
use crate::checks::jev::error::JevError;
use crate::checks::jev::plan_file::{self, AgentReason, Decider, JevCalibration, PlanCall};
use crate::checks::jev::replay;
use crate::checks::jev::verify::{self, Agreement, Evidence, Expected, JevDecision};
use crate::checks::model::{Check, CheckId};
use crate::checks::sandbox::{Sandbox, SandboxLease};

#[cfg(test)]
pub mod fake;

/// Everything one check needs planned: the check itself plus enough context
/// (repository root, root-relative `declared_in`, the owning requirement's
/// title) to run the agent and build the Jev verification request.
#[derive(Debug)]
pub struct PlanRequest {
    /// Run-unique id, mirroring [`AgentRunRequest::check_id`] — used only for
    /// session-id namespacing (see [`AgentPlanner::run_agent`]); `multi plan`
    /// has no live UI yet (MULTI-1829), so nothing routes progress by it.
    pub check_id: CheckId,
    pub check: Check,
    /// The owning requirement's title, sent to Jev as `state.requirement.title`.
    pub requirement_title: String,
    /// The declaring `CHECKS.md`'s path relative to [`Self::root`]
    /// (MULTI-1834), e.g. `services/keystore/CHECKS.md` — used both in the
    /// agent's instructions (identically to `multi check`) and as
    /// `state.requirement.declared_in` in the Jev verification request.
    pub declared_in: PathBuf,
    /// The requirement's repository root (MULTI-1834) — the real,
    /// unsandboxed source directory both the agent's sandbox and the in-host
    /// replay are rooted at.
    pub root: PathBuf,
}

/// One check's planned entry — everything
/// [`crate::checks::jev::plan_file::PlanCheck`] needs except `ordinal`, which
/// is assigned by the caller: it knows the check's position within its
/// declaring file, and a `PlannedCheck` alone (built from just a check + its
/// agent outcome) does not.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedCheck {
    pub title: String,
    pub prompt_xxh64: String,
    pub decider: Decider,
    pub verdict: bool,
    pub evidence: Option<String>,
    pub jev: Option<JevCalibration>,
    pub calls: Vec<PlanCall>,
}

impl PlannedCheck {
    /// Attach `ordinal` to build the on-disk [`plan_file::PlanCheck`].
    pub fn into_plan_check(self, ordinal: u32) -> plan_file::PlanCheck {
        plan_file::PlanCheck {
            title: self.title,
            ordinal,
            prompt_xxh64: self.prompt_xxh64,
            decider: self.decider,
            verdict: self.verdict,
            evidence: self.evidence,
            jev: self.jev,
            calls: self.calls,
        }
    }
}

/// The abstraction over planning a single check — mirrors [`CheckExecutor`]'s
/// DI convention exactly (one method, boxed alias, test fake).
#[async_trait]
pub trait Planner: Send + Sync {
    /// Run the check's agent and decide how it's frozen into a plan entry.
    async fn plan_check(&self, req: PlanRequest) -> Result<PlannedCheck>;
}

/// A boxed [`Planner`] for dynamic dispatch (DI seam).
pub type BoxedPlanner = Box<dyn Planner + Send + Sync>;

/// A [`Planner::plan_check`] failure that must abort the *whole* `multi plan`
/// run rather than merely error the one check it was raised for — see the
/// module docs' "Errors" section. `checks::plan::run` detects this via
/// `miette::Report::downcast_ref` on a failed `plan_check`'s error.
#[derive(Debug, Error, Diagnostic)]
#[error("Jev rejected the plan-time verification request: {0}")]
#[diagnostic(
    code(jev::plan::abort),
    help("fix the underlying TypeSafe credential or request problem, then re-run `multi plan`")
)]
pub(crate) struct AbortPlanRun(#[source] pub JevError);

/// The real [`Planner`]: composes a [`CheckExecutor`] (built the same way
/// `multi check` builds it — see
/// [`crate::checks::config::Resolved::build_executor`]) and owns everything
/// plan-specific: relativizing captured calls, replaying them in-host,
/// calibrating against Jev, and deciding the resulting [`Decider`].
pub struct AgentPlanner {
    executor: Arc<dyn CheckExecutor + Send + Sync>,
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    jev_client: Arc<JevClient>,
    jev_config: JevConfig,
    /// How many times to (re)run a check whose agent fails to report — the
    /// same knob `multi check`'s execution phase uses
    /// ([`crate::checks::config::Config::max_attempts`]), clamped to ≥1.
    max_attempts: usize,
}

impl AgentPlanner {
    pub fn new(
        executor: Arc<dyn CheckExecutor + Send + Sync>,
        sandbox: Arc<dyn Sandbox + Send + Sync>,
        jev_client: Arc<JevClient>,
        jev_config: JevConfig,
        max_attempts: usize,
    ) -> Self {
        Self {
            executor,
            sandbox,
            jev_client,
            jev_config,
            max_attempts: max_attempts.max(1),
        }
    }

    /// Run `req`'s agent, retrying in place (up to `max_attempts`) while it
    /// fails to report a verdict — the same policy
    /// `execution::execute_check_job` implements for `multi check`, but
    /// replicated here in miniature rather than reused directly: that
    /// function is tightly coupled to the presenter/reporting actors this
    /// pipeline doesn't have (it fires `UiEvent`s and `tell`s a
    /// `ReportingActor` inline), so reusing it would mean threading dummy
    /// actors through purely to satisfy its signature — replicating the
    /// small loop is cleaner here.
    ///
    /// Returns the sandbox root the *successful* attempt actually ran in,
    /// alongside its outcome. The root is captured by acquiring the lease
    /// **before** handing it to the executor (`SandboxLease::acquire` caches
    /// its result, so the executor's own `acquire()` call inside
    /// `run_check` reuses this clone rather than making a second one) —
    /// [`entry_from_outcome`] needs this path to relativize the outcome's
    /// captured calls, and by the time `run_check` returns, the executor has
    /// already dropped (and thereby torn down) its copy of the lease.
    ///
    /// `Err` only when every attempt finished without a verdict — the
    /// ticket: "an agent that never reports a verdict after `max_attempts`
    /// ⇒ no plan entry, reported as `error`".
    async fn run_agent(&self, req: &PlanRequest) -> Result<(PathBuf, AgentOutcome)> {
        let mut attempt: u32 = 1;
        loop {
            let lease = SandboxLease::new(self.sandbox.clone(), req.root.clone());
            let sandbox_root = lease.acquire().await?.to_path_buf();

            let request = AgentRunRequest {
                check_id: req.check_id,
                check: req.check.clone(),
                source_dir: req.root.clone(),
                sandbox: lease,
                declared_in: req.declared_in.clone(),
                attempt,
                // `multi plan` has no live UI yet (MULTI-1829): progress is
                // display-only and this pipeline has no presenter to forward
                // it to.
                progress: None,
            };

            let outcome = self.executor.run_check(request).await?;
            if outcome.has_verdict() {
                return Ok((sandbox_root, outcome));
            }
            if (attempt as usize) >= self.max_attempts {
                let stop = outcome
                    .stop_reason
                    .as_deref()
                    .map(|r| format!(" (stop: {r})"))
                    .unwrap_or_default();
                return Err(miette!(
                    "agent finished without reporting a verdict after {} attempt(s){stop}",
                    self.max_attempts,
                ));
            }
            attempt += 1;
        }
    }
}

#[async_trait]
impl Planner for AgentPlanner {
    async fn plan_check(&self, req: PlanRequest) -> Result<PlannedCheck> {
        let (sandbox_root, outcome) = self.run_agent(&req).await?;
        // Owned so `EntryContext::declared_in` can borrow a `&str` that
        // outlives the `to_string_lossy()` temporary.
        let declared_in = req.declared_in.to_string_lossy().into_owned();
        let ctx = EntryContext {
            requirement_title: &req.requirement_title,
            declared_in: &declared_in,
            root: &req.root,
            sandbox_root: &sandbox_root,
            jev_client: self.jev_client.as_ref(),
            jev_config: &self.jev_config,
        };
        entry_from_outcome(&req.check, &outcome, &ctx).await
    }
}

/// Everything [`entry_from_outcome`] needs beyond the check and its agent
/// outcome: the sandbox root the agent ran in (to relativize captured
/// calls), the real repository root (for in-host replay), the requirement
/// context Jev's state needs, and the Jev handle to calibrate against.
pub struct EntryContext<'a> {
    pub requirement_title: &'a str,
    pub declared_in: &'a str,
    pub root: &'a Path,
    pub sandbox_root: &'a Path,
    pub jev_client: &'a JevClient,
    pub jev_config: &'a JevConfig,
}

/// Turn a completed agent run into a [`PlannedCheck`] — runs no agent itself.
/// Steps: relativize the captured calls against [`EntryContext::sandbox_root`],
/// replay them in-host against [`EntryContext::root`] to compute fresh
/// checksums and detect truncation, then calibrate against Jev unless
/// calibration would be pointless (see the module docs).
///
/// This is the single entry-builder MULTI-1826's self-healing (`multi check`
/// escalation) will also call, so a healed entry is by construction
/// identical to what `multi plan` would have written. At that call site the
/// CoW sandbox is *also* already gone by the time the caller can act — this
/// function is designed for that from the start: [`EntryContext::sandbox_root`]
/// is a plain path *value*, never touched as a live directory
/// ([`plan_file::relativize`] is purely lexical — no filesystem access), so
/// it works whether or not the sandbox it names still exists on disk. The
/// alternative this module considered and rejected — relativizing inside
/// `plan_check` before the lease drops and having this function take
/// already-relative calls — would have made the "single entry builder"
/// contract asymmetric: MULTI-1826's caller would need to duplicate that
/// relativizing step itself rather than getting it from this one function.
///
/// # Preconditions
///
/// `outcome` must carry a verdict (`outcome.verdict.is_some()`); every
/// current caller only invokes this once a verdict is known to exist (an
/// agent that never reports one produces no plan entry — see
/// [`AgentPlanner::run_agent`]). Returns `Err` defensively if that
/// precondition is violated, rather than panicking.
pub async fn entry_from_outcome(
    check: &Check,
    outcome: &AgentOutcome,
    ctx: &EntryContext<'_>,
) -> Result<PlannedCheck> {
    let report = outcome
        .verdict
        .clone()
        .ok_or_else(|| miette!("entry_from_outcome called on an outcome with no verdict"))?;
    let prompt_hash = plan_file::prompt_xxh64(&check.title, &check.prompt);

    if outcome.tool_calls.is_empty() {
        return Ok(no_calibration_entry(
            check,
            &prompt_hash,
            &report,
            AgentReason::NoToolCalls,
            vec![],
        ));
    }

    // `?`: any call that fails to relativize aborts this check's planning
    // entirely rather than silently dropping the call — see
    // `replay_captured_calls`'s docs on why a partial evidence set is the
    // dangerous direction here.
    let replayed = replay_captured_calls(&outcome.tool_calls, ctx.sandbox_root, ctx.root).await?;

    if replayed.iter().any(|r| r.missing || r.truncated) {
        // The plan cannot guard against a moved/deleted file or a Grep/Glob
        // already at its result cap, so Jev is never trusted with it — and a
        // Jev call couldn't change that outcome either way, so calibration
        // is skipped entirely (keeps Jev spend minimal). A `missing` call
        // (the stored path no longer resolves at all — practically
        // unreachable this soon after the same agent successfully read it,
        // but handled defensively) is dropped from `calls` outright, since
        // the plan schema has no variant for "a call with nothing to
        // freeze"; a `truncated` call is still stored, per schema, as
        // `PlanCall::Truncated`.
        let calls = replayed
            .iter()
            .filter(|r| !r.missing)
            .map(|r| {
                if r.truncated {
                    PlanCall::Truncated {
                        tool: r.tool,
                        input: r.input.clone(),
                    }
                } else {
                    r.to_plan_call()
                        .expect("non-truncated, non-missing replay always checksums")
                }
            })
            .collect();
        return Ok(no_calibration_entry(
            check,
            &prompt_hash,
            &report,
            AgentReason::TruncatedDiscovery,
            calls,
        ));
    }

    // Every call replayed cleanly: build the frozen calls and the evidence
    // Jev sees, then calibrate.
    let calls: Vec<PlanCall> = replayed
        .iter()
        .map(|r| {
            r.to_plan_call()
                .expect("checked above: no truncated/missing calls")
        })
        .collect();
    let evidence: Vec<Evidence> = replayed
        .iter()
        .map(|r| Evidence {
            tool: r.tool,
            input: r.input.clone(),
            output: r
                .output
                .clone()
                .expect("non-missing replay always has output"),
            windowed: r.windowed,
        })
        .collect();

    let expected = if report.success {
        Expected::Pass
    } else {
        Expected::Fail
    };
    let requirement = verify::Requirement {
        title: ctx.requirement_title,
        declared_in: ctx.declared_in,
    };

    let (decider, jev) = calibrate(
        ctx.jev_client,
        ctx.jev_config,
        &requirement,
        check,
        &evidence,
        expected,
    )
    .await?;

    Ok(PlannedCheck {
        title: check.title.clone(),
        prompt_xxh64: prompt_hash,
        decider,
        verdict: report.success,
        evidence: report.evidence,
        jev,
        calls,
    })
}

/// Build a [`PlannedCheck`] for a check that skipped calibration entirely
/// (`jev: None`) — [`AgentReason::NoToolCalls`] or
/// [`AgentReason::TruncatedDiscovery`], the two reasons decided before any
/// Jev call is made.
fn no_calibration_entry(
    check: &Check,
    prompt_hash: &str,
    report: &CheckReport,
    reason: AgentReason,
    calls: Vec<PlanCall>,
) -> PlannedCheck {
    PlannedCheck {
        title: check.title.clone(),
        prompt_xxh64: prompt_hash.to_string(),
        decider: Decider::Agent(reason),
        verdict: report.success,
        evidence: report.evidence.clone(),
        jev: None,
        calls,
    }
}

/// Relativize every captured call against `sandbox_root`, then replay it
/// in-host against `root`.
///
/// A call that fails to relativize (its path escapes `sandbox_root`, or its
/// input isn't shaped the way the tool always produces it) is **not**
/// dropped — that used to be this function's behavior, and it was a bug: a
/// successfully captured call can only fail to relativize if something is
/// genuinely wrong (the live jail already rejects an out-of-sandbox path at
/// call time — see `crate::checks::executor::jail::Jailed` — and an errored
/// call is never captured in the first place, per
/// `crate::checks::executor::tool_capture`'s docs), so silently dropping it
/// would freeze an *incomplete* evidence set under the check's title as if
/// it were complete. That is the dangerous direction for this engine: Jev
/// (or a future re-planning pass) would trust a plan that looks fine but
/// omits evidence a full run actually depended on. Failing the whole check
/// instead means it is reported `error` and no entry — complete or
/// incomplete — is ever written for it (see
/// `crate::checks::plan::build_plan_files`, which also preserves whatever
/// entry already existed rather than deleting it on this kind of failure).
async fn replay_captured_calls(
    calls: &[ToolCall],
    sandbox_root: &Path,
    root: &Path,
) -> Result<Vec<replay::ReplayedCall>> {
    let mut out = Vec::with_capacity(calls.len());
    for call in calls {
        let relative = plan_file::relativize(call.tool, &call.input, sandbox_root)
            .map_err(|err| escaped_call_report(call.tool, &err))?;
        out.push(replay::replay_call(call.tool, &relative, root).await);
    }
    Ok(out)
}

/// Build the per-check diagnostic for a captured call that failed to
/// relativize (see [`replay_captured_calls`]'s docs on why this aborts the
/// check rather than dropping the call). Always names the tool; for a
/// [`plan_file::PlanError::PathEscape`], also names the offending path —
/// but **never** as an absolute host path (a sandbox clone lives under a
/// process-specific temp directory, and the escaping path may itself be an
/// arbitrary absolute host path, e.g. `/etc/passwd`): [`sanitize_escaped_path`]
/// renders it root-relative when the raw string happens to still share
/// `sandbox_root`'s own prefix (the common case — an in-bounds-looking path
/// that only escapes after `..` resolution), and a generic, non-specific
/// placeholder otherwise.
fn escaped_call_report(tool: ReadOnlyTool, err: &plan_file::PlanError) -> miette::Report {
    match err {
        plan_file::PlanError::PathEscape { path, root } => {
            let sanitized = sanitize_escaped_path(path, root);
            miette!(
                "a captured `{tool:?}` call's path escaped the repository root ({sanitized}); \
                 this should be unreachable (the sandbox jail rejects an escape at call time) — \
                 treating this check as unplannable rather than freezing incomplete evidence"
            )
        }
        other => miette!(
            "a captured `{tool:?}` call could not be relativized against the repository root \
             ({other}); treating this check as unplannable rather than freezing incomplete evidence"
        ),
    }
}

/// Render a [`plan_file::PlanError::PathEscape`]'s raw `path` safely for a
/// diagnostic — see [`escaped_call_report`]. Strips `root`'s own string form
/// as a literal prefix when `path` happens to start with it (the escape is
/// then some interior `..` climbing back out, so the stripped suffix is
/// root-relative and safe to print); otherwise `path` shares nothing with
/// `root` at all (e.g. an unrelated absolute host path), and this returns a
/// placeholder that names no host-specific detail.
fn sanitize_escaped_path(path: &str, root: &Path) -> String {
    let root_str = root.to_string_lossy();
    match path.strip_prefix(root_str.as_ref()) {
        Some(rest) => format!("<repository root>{rest}"),
        None => "<a path outside the repository root>".to_string(),
    }
}

/// Ask Jev to reproduce the agent's verdict over the replayed evidence, then
/// — unless that call is already [`JevDecision::OverBudget`] — run the
/// empty-evidence negative control, and decide the [`Decider`] from both
/// (see [`decide_from_calibration`]).
async fn calibrate(
    client: &JevClient,
    config: &JevConfig,
    requirement: &verify::Requirement<'_>,
    check: &Check,
    evidence: &[Evidence],
    expected: Expected,
) -> Result<(Decider, Option<JevCalibration>)> {
    let main = verify::verify(client, config, requirement, check, evidence)
        .await
        .map_err(jev_error_to_report)?;

    if matches!(main, JevDecision::OverBudget) {
        // The control carries strictly less evidence than the main request,
        // which was already over budget — it cannot possibly fit either, so
        // it's skipped: a Jev call that cannot change the outcome.
        return Ok((Decider::Agent(AgentReason::OverBudget), None));
    }

    let control = verify::verify(client, config, requirement, check, &[])
        .await
        .map_err(jev_error_to_report)?;

    Ok(decide_from_calibration(
        expected,
        config.threshold,
        &main,
        &control,
    ))
}

/// Map a failed Jev request to a [`miette::Report`] — either an
/// [`AbortPlanRun`] (`crate::checks::plan::run` detects this via
/// `downcast_ref` and aborts the whole run) or a plain per-check diagnostic.
/// See the module docs' "Errors" section for which variants go where and why.
fn jev_error_to_report(err: JevError) -> miette::Report {
    match err {
        JevError::Unauthorized | JevError::MissingApiKey | JevError::Invalid { .. } => {
            miette::Report::new(AbortPlanRun(err))
        }
        JevError::Exhausted { .. } | JevError::Transport(_) => miette::Report::new(err),
    }
}

/// Pure classification (no I/O) — see the module docs' "Calibration" section
/// for the full precedence. `main`/`control` are guaranteed non-`OverBudget`
/// by [`calibrate`]'s caller (an `OverBudget` main short-circuits before a
/// control is ever requested); kept as `&JevDecision` (rather than a
/// narrower `(f64, String, Option<ChoiceOutcome>)` tuple) so this stays
/// directly testable against `verify::verify`'s own return type.
fn decide_from_calibration(
    expected: Expected,
    threshold: f64,
    main: &JevDecision,
    control: &JevDecision,
) -> (Decider, Option<JevCalibration>) {
    let (main_noul, model, choice) = match main {
        JevDecision::Satisfied {
            noul,
            model,
            choice,
        }
        | JevDecision::NotVerified {
            noul,
            model,
            choice,
        } => (*noul, model.clone(), *choice),
        JevDecision::OverBudget => {
            // Unreachable through `calibrate` (see above) — kept total
            // rather than `unreachable!()` so a future/direct caller that
            // does pass an `OverBudget` main fails safe (agent-only)
            // instead of panicking.
            return (Decider::Agent(AgentReason::OverBudget), None);
        }
    };

    let agreement_main = verify::agreement(expected, main_noul, threshold);
    // Never fabricated: `None` when Jev's Choice answer was missing or
    // malformed (`verify::ChoiceOutcome` — "Choice never gates" — allows
    // this), recorded as `None` (the field is omitted from the rendered
    // plan entirely) rather than a synthesized stand-in for what Jev didn't
    // actually say (MULTI-1824 review).
    let reading = choice.map(|c| c.reading);

    let control_noul = match control {
        JevDecision::Satisfied { noul, .. } | JevDecision::NotVerified { noul, .. } => Some(*noul),
        JevDecision::OverBudget => None,
    };

    let Some(control_noul) = control_noul else {
        // The negative control itself came back over budget — near
        // impossible in practice (it carries strictly less evidence than
        // the main request, which already fit), but handled: we cannot
        // confirm the control rejected, so the check stays agent-decided;
        // there is no complete (noul, control_noul) pair to record.
        return (Decider::Agent(AgentReason::ControlFailed), None);
    };

    let agreement_control = verify::agreement(Expected::Fail, control_noul, threshold);
    let calibration = JevCalibration {
        model,
        noul: main_noul,
        control_noul,
        reading,
    };

    let reason = match agreement_main {
        Agreement::Disagrees => Some(AgentReason::JevDisagreed),
        Agreement::Uncertain => Some(AgentReason::JevUncertain),
        Agreement::Agrees if agreement_control != Agreement::Agrees => {
            Some(AgentReason::ControlFailed)
        }
        Agreement::Agrees => None,
    };

    match reason {
        Some(reason) => (Decider::Agent(reason), Some(calibration)),
        None => (Decider::Jev, Some(calibration)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use static_assertions::assert_obj_safe;
    use tempfile::TempDir;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::checks::executor::{FakeExecutor, ReadOnlyTool as Tool};
    use crate::checks::jev::error::TYPESAFE_API_KEY_VAR;
    use crate::checks::jev::plan_file::Reading as R;
    use crate::checks::jev::verify::ChoiceOutcome;
    use crate::checks::model::Check;
    use crate::checks::sandbox::RecordingSandbox;

    assert_obj_safe!(Planner);

    // -- fixtures ---------------------------------------------------------

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

    fn satisfied(noul: f64) -> JevDecision {
        JevDecision::Satisfied {
            noul,
            model: "jev-1.13.0".to_string(),
            choice: None,
        }
    }

    fn not_verified(noul: f64) -> JevDecision {
        JevDecision::NotVerified {
            noul,
            model: "jev-1.13.0".to_string(),
            choice: None,
        }
    }

    // -- decide_from_calibration: pure precedence tests --------------------

    #[test]
    fn agreement_on_both_sides_decides_jev() {
        let (decider, jev) =
            decide_from_calibration(Expected::Pass, 0.75, &satisfied(0.9), &not_verified(0.1));
        assert_eq!(decider, Decider::Jev);
        let jev = jev.expect("calibration recorded");
        assert_eq!(jev.noul, 0.9);
        assert_eq!(jev.control_noul, 0.1);
    }

    #[test]
    fn main_disagreement_is_agent_with_jev_disagreed() {
        // Expected Pass, but noul is low (agrees with Fail instead).
        let (decider, jev) =
            decide_from_calibration(Expected::Pass, 0.75, &satisfied(0.1), &not_verified(0.1));
        assert_eq!(decider, Decider::Agent(AgentReason::JevDisagreed));
        assert!(jev.is_some(), "calibration still recorded for diagnosis");
    }

    #[test]
    fn main_uncertain_is_agent_with_jev_uncertain() {
        // threshold=0.75, low=0.25: 0.5 is inside the band for either
        // expectation.
        let (decider, _) =
            decide_from_calibration(Expected::Pass, 0.75, &satisfied(0.5), &not_verified(0.1));
        assert_eq!(decider, Decider::Agent(AgentReason::JevUncertain));
    }

    /// Ticket-called-out case: an agent FAIL with `low < noul < threshold` is
    /// uncertain, not a disagreement.
    #[test]
    fn agent_fail_with_noul_inside_the_band_is_uncertain() {
        let (decider, _) =
            decide_from_calibration(Expected::Fail, 0.75, &satisfied(0.5), &not_verified(0.1));
        assert_eq!(decider, Decider::Agent(AgentReason::JevUncertain));
    }

    #[test]
    fn main_agrees_but_control_fails_is_agent_with_control_failed() {
        // Main agrees (noul high, expected Pass); control noul is also high
        // (>= threshold), so the empty-evidence control did NOT reject.
        let (decider, jev) =
            decide_from_calibration(Expected::Pass, 0.75, &satisfied(0.9), &satisfied(0.9));
        assert_eq!(decider, Decider::Agent(AgentReason::ControlFailed));
        assert_eq!(jev.unwrap().control_noul, 0.9);
    }

    #[test]
    fn control_agreement_uses_the_same_band_as_main() {
        // control_noul strictly between low and threshold: uncertain, which
        // counts as "did not agree" (control failed), matching the ticket's
        // literal "must return noul <= low" via the agreement band.
        let (decider, _) =
            decide_from_calibration(Expected::Pass, 0.75, &satisfied(0.9), &satisfied(0.5));
        assert_eq!(decider, Decider::Agent(AgentReason::ControlFailed));
    }

    #[test]
    fn over_budget_main_short_circuits_to_agent_over_budget() {
        let (decider, jev) = decide_from_calibration(
            Expected::Pass,
            0.75,
            &JevDecision::OverBudget,
            &not_verified(0.1),
        );
        assert_eq!(decider, Decider::Agent(AgentReason::OverBudget));
        assert!(jev.is_none());
    }

    #[test]
    fn over_budget_control_is_agent_with_control_failed_and_no_calibration() {
        let (decider, jev) = decide_from_calibration(
            Expected::Pass,
            0.75,
            &satisfied(0.9),
            &JevDecision::OverBudget,
        );
        assert_eq!(decider, Decider::Agent(AgentReason::ControlFailed));
        assert!(jev.is_none(), "no complete (noul, control_noul) pair");
    }

    /// MULTI-1824 review: a plan must never claim Jev said something it
    /// didn't — when the record-only Choice answer is missing (both
    /// `satisfied`/`not_verified` helpers script `choice: None`), the
    /// recorded `reading` must be `None`, never a noul-band-derived
    /// stand-in.
    #[test]
    fn reading_is_none_when_the_choice_answer_is_missing() {
        let (_, jev) =
            decide_from_calibration(Expected::Pass, 0.75, &satisfied(0.9), &not_verified(0.1));
        assert_eq!(
            jev.unwrap().reading,
            None,
            "never fabricate a reading Jev didn't actually give"
        );
    }

    #[test]
    fn reading_records_the_actual_choice_answer_when_present() {
        let main = JevDecision::Satisfied {
            noul: 0.9,
            model: "jev-1.13.0".to_string(),
            choice: Some(ChoiceOutcome {
                reading: R::Violated,
                confidence: 0.9,
            }),
        };
        let (_, jev) = decide_from_calibration(Expected::Pass, 0.75, &main, &not_verified(0.1));
        assert_eq!(jev.unwrap().reading, Some(R::Violated));
    }

    // -- entry_from_outcome: no-tool-calls / truncated short circuits ------

    fn outcome_with_calls(success: bool, calls: Vec<ToolCall>) -> AgentOutcome {
        AgentOutcome {
            verdict: Some(CheckReport {
                success,
                evidence: Some("agent evidence".to_string()),
            }),
            stop_reason: None,
            turns: 1,
            error: None,
            trace_jsonl: None,
            tool_calls: calls,
        }
    }

    fn ctx<'a>(
        requirement_title: &'a str,
        declared_in: &'a str,
        root: &'a Path,
        sandbox_root: &'a Path,
        jev_client: &'a JevClient,
        jev_config: &'a JevConfig,
    ) -> EntryContext<'a> {
        EntryContext {
            requirement_title,
            declared_in,
            root,
            sandbox_root,
            jev_client,
            jev_config,
        }
    }

    #[tokio::test]
    async fn entry_from_outcome_requires_a_verdict() {
        let dir = TempDir::new().unwrap();
        let client = JevClient::new("https://unused.invalid").unwrap();
        let cfg = jev_config(0.75, "https://unused.invalid");
        let outcome = AgentOutcome::default();
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);
        assert!(entry_from_outcome(&check(), &outcome, &ec).await.is_err());
    }

    #[tokio::test]
    async fn zero_tool_calls_skips_calibration_and_is_no_tool_calls() {
        let dir = TempDir::new().unwrap();
        let client = JevClient::new("https://unused.invalid").unwrap();
        let cfg = jev_config(0.75, "https://unused.invalid");
        let outcome = outcome_with_calls(true, vec![]);
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);

        let planned = entry_from_outcome(&check(), &outcome, &ec).await.unwrap();
        assert_eq!(planned.decider, Decider::Agent(AgentReason::NoToolCalls));
        assert!(planned.jev.is_none());
        assert!(planned.calls.is_empty());
    }

    #[tokio::test]
    async fn truncated_call_skips_calibration_and_is_truncated_discovery() {
        let dir = TempDir::new().unwrap();
        for i in 0..260 {
            std::fs::write(dir.path().join(format!("f{i:04}.txt")), "NEEDLE\n").unwrap();
        }
        let client = JevClient::new("https://unused.invalid").unwrap();
        let cfg = jev_config(0.75, "https://unused.invalid");
        let calls = vec![ToolCall {
            tool: Tool::Grep,
            input: json!({"pattern": "NEEDLE", "path": dir.path().to_string_lossy()}),
        }];
        let outcome = outcome_with_calls(true, calls);
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);

        let planned = entry_from_outcome(&check(), &outcome, &ec).await.unwrap();
        assert_eq!(
            planned.decider,
            Decider::Agent(AgentReason::TruncatedDiscovery)
        );
        assert!(planned.jev.is_none());
        assert_eq!(planned.calls.len(), 1);
        assert!(matches!(planned.calls[0], PlanCall::Truncated { .. }));
    }

    // -- entry_from_outcome: an escaping captured call errors the check ----

    /// MULTI-1824 review (blocker): a captured call that fails to
    /// relativize must error the whole check, never be silently dropped —
    /// dropping it would freeze an incomplete evidence set under the
    /// check's title as if it were complete, which is the dangerous
    /// direction for this engine.
    #[tokio::test]
    async fn a_captured_call_that_escapes_the_sandbox_root_errors_the_check_not_drops_the_call() {
        let dir = TempDir::new().unwrap();
        let client = JevClient::new("https://unused.invalid").unwrap();
        let cfg = jev_config(0.75, "https://unused.invalid");
        let calls = vec![ToolCall {
            tool: Tool::Read,
            input: json!({"file_path": "/etc/passwd"}),
        }];
        let outcome = outcome_with_calls(true, calls);
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);

        let err = entry_from_outcome(&check(), &outcome, &ec)
            .await
            .expect_err("an escaping captured call must error the check, not drop it");
        let message = err.to_string();
        assert!(message.contains("Read"), "names the tool: {message}");
        assert!(
            !message.contains(dir.path().to_string_lossy().as_ref()),
            "must never leak the sandbox's absolute host path: {message}"
        );
    }

    /// The sanitized-but-informative half of the same fix: when the
    /// escaping path happens to still share the sandbox root's own prefix
    /// (a `..` climb rather than a wholly unrelated absolute path), the
    /// diagnostic renders it root-relative — never the raw absolute host
    /// path.
    #[tokio::test]
    async fn an_escape_sharing_the_root_prefix_renders_root_relative_not_absolute() {
        let dir = TempDir::new().unwrap();
        let client = JevClient::new("https://unused.invalid").unwrap();
        let cfg = jev_config(0.75, "https://unused.invalid");
        let escaping = format!("{}/../outside.txt", dir.path().display());
        let calls = vec![ToolCall {
            tool: Tool::Read,
            input: json!({"file_path": escaping}),
        }];
        let outcome = outcome_with_calls(true, calls);
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);

        let err = entry_from_outcome(&check(), &outcome, &ec)
            .await
            .expect_err("an escaping captured call must error the check");
        let message = err.to_string();
        assert!(
            message.contains("<repository root>"),
            "root-relative rendering: {message}"
        );
        assert!(
            !message.contains(dir.path().to_string_lossy().as_ref()),
            "must never leak the sandbox's absolute host path: {message}"
        );
    }

    // -- entry_from_outcome: full calibration path (wiremock Jev) -----------

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

    /// Mount two scripted responses on `server`, distinguished by whether the
    /// request body's `state.evidence` array is empty (the negative control)
    /// or not (the main verdict question).
    async fn mock_calibration(server: &MockServer, main_noul: f64, control_noul: f64) {
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(body_string_contains("\"evidence\":[]"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "jev-1.13.0",
                "answers": {
                    "satisfied": {"type": "noul", "noul": control_noul},
                },
                "usage": {"input_tokens": 10, "output_tokens": 5},
            })))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "jev-1.13.0",
                "answers": {
                    "satisfied": {"type": "noul", "noul": main_noul},
                },
                "usage": {"input_tokens": 200, "output_tokens": 20},
            })))
            .mount(server)
            .await;
    }

    fn write_file(dir: &Path, relative: &str, content: &str) {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[tokio::test]
    async fn full_calibration_path_agrees_and_decides_jev() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/lib.rs", "fn main() {}\n");

        let server = MockServer::start().await;
        mock_calibration(&server, 0.95, 0.02).await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = jev_config(0.75, &server.uri());

        let calls = vec![ToolCall {
            tool: Tool::Read,
            input: json!({"file_path": dir.path().join("src/lib.rs").to_string_lossy()}),
        }];
        let outcome = outcome_with_calls(true, calls);
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);

        let c = check();
        let planned = with_api_key(|| entry_from_outcome(&c, &outcome, &ec))
            .await
            .unwrap();
        assert_eq!(planned.decider, Decider::Jev);
        let jev = planned.jev.unwrap();
        assert_eq!(jev.noul, 0.95);
        assert_eq!(jev.control_noul, 0.02);
        assert_eq!(planned.calls.len(), 1);
    }

    #[tokio::test]
    async fn full_calibration_path_control_fails() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/lib.rs", "fn main() {}\n");

        let server = MockServer::start().await;
        // Main agrees, but control also answers "yes" (bias-prone question).
        mock_calibration(&server, 0.95, 0.9).await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = jev_config(0.75, &server.uri());

        let calls = vec![ToolCall {
            tool: Tool::Read,
            input: json!({"file_path": dir.path().join("src/lib.rs").to_string_lossy()}),
        }];
        let outcome = outcome_with_calls(true, calls);
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);

        let c = check();
        let planned = with_api_key(|| entry_from_outcome(&c, &outcome, &ec))
            .await
            .unwrap();
        assert_eq!(planned.decider, Decider::Agent(AgentReason::ControlFailed));
    }

    #[tokio::test]
    async fn unauthorized_jev_error_aborts_via_abort_plan_run() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/lib.rs", "fn main() {}\n");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = jev_config(0.75, &server.uri());

        let calls = vec![ToolCall {
            tool: Tool::Read,
            input: json!({"file_path": dir.path().join("src/lib.rs").to_string_lossy()}),
        }];
        let outcome = outcome_with_calls(true, calls);
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);

        let c = check();
        let err = with_api_key(|| entry_from_outcome(&c, &outcome, &ec))
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<AbortPlanRun>().is_some());
    }

    #[tokio::test]
    async fn transport_jev_error_is_a_plain_per_check_error() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/lib.rs", "fn main() {}\n");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = jev_config(0.75, &server.uri());

        let calls = vec![ToolCall {
            tool: Tool::Read,
            input: json!({"file_path": dir.path().join("src/lib.rs").to_string_lossy()}),
        }];
        let outcome = outcome_with_calls(true, calls);
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);

        let c = check();
        let err = with_api_key(|| entry_from_outcome(&c, &outcome, &ec))
            .await
            .unwrap_err();
        assert!(err.downcast_ref::<AbortPlanRun>().is_none());
    }

    // -- plan_check == entry_from_outcome -----------------------------------

    /// MULTI-1824 acceptance: `plan_check`'s entry equals `entry_from_outcome`
    /// of the same outcome. Drives a real `AgentPlanner` (over a
    /// `FakeExecutor` and a `RecordingSandbox`) and an independent, direct
    /// `entry_from_outcome` call over the exact same scripted outcome, and
    /// asserts the two `PlannedCheck`s match field for field.
    #[tokio::test]
    async fn plan_checks_entry_equals_entry_from_outcome_of_the_same_outcome() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/lib.rs", "fn main() {}\n");

        let server = MockServer::start().await;
        mock_calibration(&server, 0.95, 0.02).await;
        let client = Arc::new(JevClient::new(server.uri()).unwrap());
        let cfg = jev_config(0.75, &server.uri());

        let calls = vec![ToolCall {
            tool: Tool::Read,
            input: json!({"file_path": dir.path().join("src/lib.rs").to_string_lossy()}),
        }];
        let executor = Arc::new(
            FakeExecutor::new()
                .with_report(0, true, Some("agent evidence"))
                .with_tool_calls(0, calls.clone()),
        );
        let sandbox = Arc::new(RecordingSandbox::new());

        let planner = AgentPlanner::new(
            executor.clone(),
            sandbox.clone(),
            client.clone(),
            cfg.clone(),
            3,
        );

        let req = PlanRequest {
            check_id: 0,
            check: check(),
            requirement_title: "R".to_string(),
            declared_in: PathBuf::from("CHECKS.md"),
            root: dir.path().to_path_buf(),
        };

        let via_plan_check = with_api_key(|| planner.plan_check(req)).await.unwrap();

        // Independently reconstruct the outcome `plan_check` would have
        // produced and call `entry_from_outcome` on it directly.
        let outcome = outcome_with_calls(true, calls);
        // `RecordingSandbox` (like `NoopSandbox`) hands back the source path
        // itself as the "sandbox root" — the same root `plan_check`'s lease
        // acquired.
        let ec = ctx("R", "CHECKS.md", dir.path(), dir.path(), &client, &cfg);
        let c = check();
        let via_entry_from_outcome = with_api_key(|| entry_from_outcome(&c, &outcome, &ec))
            .await
            .unwrap();

        assert_eq!(via_plan_check, via_entry_from_outcome);
    }

    #[tokio::test]
    async fn plan_check_errors_when_the_agent_never_reports_after_max_attempts() {
        let dir = TempDir::new().unwrap();
        let client = Arc::new(JevClient::new("https://unused.invalid").unwrap());
        let cfg = jev_config(0.75, "https://unused.invalid");
        let executor = Arc::new(FakeExecutor::new().with_silent(0));
        let sandbox = Arc::new(RecordingSandbox::new());
        let planner = AgentPlanner::new(executor, sandbox, client, cfg, 2);

        let req = PlanRequest {
            check_id: 0,
            check: check(),
            requirement_title: "R".to_string(),
            declared_in: PathBuf::from("CHECKS.md"),
            root: dir.path().to_path_buf(),
        };

        let err = planner.plan_check(req).await.unwrap_err();
        assert!(err.to_string().contains("without reporting a verdict"));
    }

    #[tokio::test]
    async fn plan_check_retries_until_the_agent_reports() {
        let dir = TempDir::new().unwrap();
        write_file(dir.path(), "src/lib.rs", "fn main() {}\n");

        let server = MockServer::start().await;
        mock_calibration(&server, 0.95, 0.02).await;
        let client = Arc::new(JevClient::new(server.uri()).unwrap());
        let cfg = jev_config(0.75, &server.uri());

        let calls = vec![ToolCall {
            tool: Tool::Read,
            input: json!({"file_path": dir.path().join("src/lib.rs").to_string_lossy()}),
        }];
        let executor = Arc::new(
            FakeExecutor::new()
                .with_silent_until(0, 2, true, Some("ok now"))
                .with_tool_calls(0, calls),
        );
        let sandbox = Arc::new(RecordingSandbox::new());
        let planner = AgentPlanner::new(executor.clone(), sandbox, client, cfg, 3);

        let req = PlanRequest {
            check_id: 0,
            check: check(),
            requirement_title: "R".to_string(),
            declared_in: PathBuf::from("CHECKS.md"),
            root: dir.path().to_path_buf(),
        };

        let planned = with_api_key(|| planner.plan_check(req)).await.unwrap();
        assert!(planned.verdict);
        assert_eq!(executor.seen_attempts(), vec![(0, 1), (0, 2)]);
    }
}
