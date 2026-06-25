//! The real, in-process [`CheckExecutor`] (MULTI-1367): run each check as a
//! `cersei_agent::Agent` in its CoW sandbox, capturing the verdict through a
//! per-check judge tool — no `claude -p` subprocess, no MCP endpoints.

use std::time::Duration;

use async_trait::async_trait;
use cersei_agent::Agent;
use cersei_tools::permissions::AllowReadOnly;
use cersei_tools::{Tool, clear_session_shell_state};
use cersei_types::CerseiError;
use miette::{Result, miette};
use tokio_util::sync::CancellationToken;

use super::judge::{JudgeTool, VerdictSink};
use super::{
    AgentOutcome, AgentRunRequest, CheckExecutor, assemble_instructions, judge_tool_directive,
};
use crate::checks::config::Effort;
use crate::checks::config::ProviderFactory;

/// How many agentic turns a check may take before it is treated as
/// "finished without reporting". Generous: the reasoning checks this feature
/// exists for explore several files before concluding.
const MAX_TURNS: u32 = 30;

/// Runs each check by driving an in-process cersei agent. Model/provider/effort
/// come from injected configuration (see [`crate::checks::config::Config`]),
/// never hardcoded here.
pub struct CerseiExecutor {
    /// Builds a fresh provider handle per check. cersei's `Agent` takes an owned
    /// `Box<dyn Provider>`, and checks run concurrently, so we cannot share one
    /// handle — the factory mints one per run from the resolved credentials.
    factory: ProviderFactory,
    /// The concrete model ID to run (e.g. `claude-sonnet-4-6`).
    model: String,
    /// The effort level, mapped to a sampling temperature (see
    /// [`effort_temperature`]).
    effort: Effort,
    /// Per-agent wall-clock timeout; on expiry the run is dropped (which stops
    /// the in-process agent) and the check resolves as errored.
    timeout: Duration,
}

impl CerseiExecutor {
    pub fn new(factory: ProviderFactory, model: String, effort: Effort, timeout: Duration) -> Self {
        Self {
            factory,
            model,
            effort,
            timeout,
        }
    }
}

/// The read-only tool set a verification agent gets by default: observe, do not
/// mutate. Execution-requiring checks (which would need Bash/Write) are gated
/// separately and are future work — the default is least privilege.
fn read_only_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(cersei_tools::file_read::FileReadTool),
        Box::new(cersei_tools::grep_tool::GrepTool),
        Box::new(cersei_tools::glob_tool::GlobTool),
    ]
}

/// Map our coarse [`Effort`] onto a sampling temperature.
///
/// Extended thinking would be the natural effort vehicle, but cersei-provider
/// 0.1.9 cannot round-trip Anthropic *thinking-block signatures*: its SSE parser
/// drops `signature_delta`, so the thinking block it sends back on the second
/// turn carries an empty signature and the API rejects it
/// (`Invalid signature in thinking block`). Until that is fixed upstream
/// (https://github.com/pacifio/cersei/issues/21) we leave thinking disabled and
/// apply effort as temperature instead — lower effort is more deterministic,
/// higher effort more exploratory.
fn effort_temperature(effort: Effort) -> f32 {
    match effort {
        Effort::Low => 0.0,
        Effort::Medium => 0.5,
        Effort::High => 1.0,
    }
}

#[async_trait]
impl CheckExecutor for CerseiExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        // Distinct session id per check: cersei's BashTool persists shell cwd/env
        // in a process-global registry keyed by session_id, so a shared id would
        // let parallel agents clobber each other's shell state.
        let session_id = format!("multi-check-{}", req.check_id);

        tracing::debug!(
            check_id = req.check_id,
            model = %self.model,
            effort = ?self.effort,
            session_id = %session_id,
            "dispatching in-process cersei check",
        );

        let provider = self.factory.build()?;

        // The judge tool and the agent share a cancellation token: a recorded
        // verdict cancels the agent so `run` returns the instant the check is
        // decided, instead of burning the remaining turn budget.
        let sink = VerdictSink::new();
        let cancel = CancellationToken::new();
        let judge = JudgeTool::new(sink.clone(), cancel.clone());

        let instructions = assemble_instructions(&req.check, &judge_tool_directive());

        let agent = Agent::builder()
            .provider_boxed(provider)
            .model(self.model.clone())
            .working_dir(req.working_dir.clone())
            .session_id(session_id.clone())
            // Least privilege: read-only tools + a policy that denies anything
            // above ReadOnly (defense in depth if the tool set ever widens).
            .permission_policy(AllowReadOnly)
            .tools(read_only_tools())
            .tool(judge)
            // Thinking is intentionally left disabled (see `effort_temperature`).
            .temperature(effort_temperature(self.effort))
            .max_turns(MAX_TURNS)
            .cancel_token(cancel.clone())
            .build()
            .map_err(|e| miette!("building check agent: {e}"))?;

        let result = tokio::time::timeout(self.timeout, agent.run(&instructions)).await;

        // Clear this session's shell state so a retry (same id) or a later run
        // never inherits stale cwd/env from the global registry.
        clear_session_shell_state(&session_id);

        // The judge slot is authoritative: if a verdict landed, the run finished
        // cleanly regardless of how `run` returned (we cancel it post-report,
        // which surfaces as `CerseiError::Cancelled`).
        let verdict = sink.verdict();

        let outcome = match result {
            Ok(Ok(output)) => AgentOutcome {
                verdict,
                stop_reason: Some(format!("{:?}", output.stop_reason)),
                turns: output.turns,
                error: None,
            },
            Ok(Err(err)) => {
                let reported = verdict.is_some();
                AgentOutcome {
                    verdict,
                    // Our own post-report cancellation is not an error.
                    stop_reason: matches!(err, CerseiError::Cancelled)
                        .then(|| "cancelled".to_string()),
                    turns: 0,
                    error: (!reported).then(|| err.to_string()),
                }
            }
            Err(_elapsed) => AgentOutcome {
                // A verdict may have landed in the instant before the timeout.
                verdict,
                stop_reason: None,
                turns: 0,
                error: Some(format!("agent timed out after {:?}", self.timeout)),
            },
        };

        Ok(outcome)
    }
}
