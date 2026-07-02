//! The real, in-process [`CheckExecutor`] (MULTI-1367): run each check as a
//! `cersei_agent::Agent` in its CoW sandbox, capturing the verdict through a
//! per-check judge tool — no `claude -p` subprocess, no MCP endpoints.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cersei_agent::Agent;
use cersei_memory::claudemd;
use cersei_tools::permissions::AllowReadOnly;
use cersei_tools::{Tool, clear_session_shell_state};
use cersei_types::CerseiError;
use miette::{Result, miette};
use tokio_util::sync::CancellationToken;

use super::jail::Jailed;
use super::judge::{JudgeTool, VerdictSink};
use super::trace::{TraceHeader, TraceRecorder, serialize_trace};
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
    /// Whether to capture each execution's agent session as a trace (returned in
    /// [`AgentOutcome::trace_jsonl`]). Driven by `multi check --trace-archive`.
    capture_traces: bool,
}

impl CerseiExecutor {
    pub fn new(
        factory: ProviderFactory,
        model: String,
        effort: Effort,
        timeout: Duration,
        capture_traces: bool,
    ) -> Self {
        Self {
            factory,
            model,
            effort,
            timeout,
            capture_traces,
        }
    }
}

/// The read-only tool set a verification agent gets by default: observe, do not
/// mutate. Execution-requiring checks (which would need Bash/Write) are gated
/// separately and are future work — the default is least privilege.
///
/// Each tool is [`Jailed`] to the agent's working directory: "read-only" alone
/// still allowed reading anywhere the user can, which let lost agents launch
/// unbounded globs over the host filesystem (timeouts) and grade the live
/// repository instead of the sandbox (postmortem C5). The jail turns an
/// out-of-sandbox path into an immediate tool error that steers the agent back.
fn read_only_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(Jailed::path_keys(
            cersei_tools::file_read::FileReadTool,
            &["file_path"],
        )),
        Box::new(Jailed::path_keys(
            cersei_tools::grep_tool::GrepTool,
            &["path"],
        )),
        Box::new(Jailed::glob(
            cersei_tools::glob_tool::GlobTool,
            &["path"],
            "pattern",
        )),
    ]
}

/// Load the project's `CLAUDE.md` hierarchy (managed `~/.claude/rules/*.md`,
/// user `~/.claude/CLAUDE.md`, project `{root}/CLAUDE.md`, local
/// `{root}/.claude/CLAUDE.md`, with `@include` expansion) as a single system
/// prompt string, or `None` if no instruction files were found.
///
/// Calls `claudemd` directly rather than going through
/// `cersei_memory::manager::MemoryManager` so this stays a stateless
/// filesystem read — no session storage, no graph memory, no multi-session
/// persistence gets pulled in as a side effect.
fn project_instructions(project_root: &Path) -> Option<String> {
    let files = claudemd::load_all_memory_files(project_root);
    let prompt = claudemd::build_memory_prompt(&files);
    (!prompt.trim().is_empty()).then_some(prompt)
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

/// The sampling temperature for a given attempt: the effort base, raised by
/// 0.5 per retry and capped at 1.0. At effort=low the base is 0.0, and a
/// temperature-0 retry is a replay: the 2026-07-01 postmortem caught one check
/// reproducing its fatal trajectory near-verbatim on all three attempts. A
/// retry has to sample differently to be worth its wall-clock.
fn attempt_temperature(effort: Effort, attempt: u32) -> f32 {
    let base = effort_temperature(effort);
    (base + 0.5 * attempt.saturating_sub(1) as f32).min(1.0)
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

        let instructions = assemble_instructions(
            &req.check,
            &judge_tool_directive(),
            &req.working_dir,
            req.attempt,
        );

        let mut agent_builder = Agent::builder()
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
            .temperature(attempt_temperature(self.effort, req.attempt))
            .max_turns(MAX_TURNS)
            .cancel_token(cancel.clone());

        // `.system_prompt()`, not `.append_system_prompt()`: cersei's agent
        // runner only ever reads `Agent.system_prompt` when building each
        // completion request (`append_system_prompt` is exclusively consumed
        // by the separate `cersei_agent::system_prompt::build_system_prompt`
        // composer, which this executor doesn't use), and we don't set a base
        // system prompt anywhere else here.
        if let Some(project_prompt) = project_instructions(&req.working_dir) {
            agent_builder = agent_builder.system_prompt(project_prompt);
        }

        // When trace capture is on, tee every agent event into a recorder.
        // `emit` invokes this synchronously for each event *before* the loop's
        // early returns, so the trace survives post-verdict cancellation and the
        // drop-on-timeout below (which cersei's own session persistence would
        // miss). The executor owns a clone, so a dropped agent doesn't lose it.
        let recorder = self.capture_traces.then(|| Arc::new(TraceRecorder::new()));
        if let Some(recorder) = &recorder {
            let recorder = Arc::clone(recorder);
            agent_builder = agent_builder.on_event(move |event| recorder.record(event));
        }

        let agent = agent_builder
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

        let mut outcome = match result {
            Ok(Ok(output)) => AgentOutcome {
                verdict,
                stop_reason: Some(format!("{:?}", output.stop_reason)),
                turns: output.turns,
                error: None,
                trace_jsonl: None,
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
                    trace_jsonl: None,
                }
            }
            Err(_elapsed) => AgentOutcome {
                // A verdict may have landed in the instant before the timeout.
                verdict,
                stop_reason: None,
                turns: 0,
                error: Some(format!("agent timed out after {:?}", self.timeout)),
                trace_jsonl: None,
            },
        };

        // Render the captured session (if any) now that the outcome is known, so
        // the trace's footer carries the authoritative verdict/stop/error.
        if let Some(recorder) = &recorder {
            let header = TraceHeader {
                check_id: req.check_id,
                check_title: &req.check.title,
                model: &self.model,
                effort: self.effort,
                working_dir: &req.working_dir,
                session_id: &session_id,
            };
            let bytes = serialize_trace(recorder, &header, &outcome);
            outcome.trace_jsonl = Some(bytes);
        }

        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_raise_the_temperature_up_to_the_cap() {
        assert_eq!(attempt_temperature(Effort::Low, 1), 0.0);
        assert_eq!(attempt_temperature(Effort::Low, 2), 0.5);
        assert_eq!(attempt_temperature(Effort::Low, 3), 1.0);
        assert_eq!(attempt_temperature(Effort::Medium, 2), 1.0);
        // Already at the cap: retries must not push past valid API range.
        assert_eq!(attempt_temperature(Effort::High, 3), 1.0);
    }
}
