//! The legacy shell-out [`CheckExecutor`]: invoke the Claude Code CLI via
//! `claude -p`. Retained only as a **migration fallback** (selectable with
//! `--executor claude`) so its verdicts can be compared against the in-process
//! [`super::cersei::CerseiExecutor`] until cersei is validated, then retired.
//!
//! With the in-process MCP result server removed (MULTI-1367), this path can no
//! longer report through a localhost tool endpoint. Instead the agent is told to
//! write its verdict as JSON to a per-check sentinel file in the sandbox, which
//! the executor reads after the process exits. This is a deliberately simpler,
//! less-trustworthy channel than the in-process judge tool — acceptable for a
//! soon-to-be-removed fallback.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};
use tokio::process::Command;

use super::judge::CheckReport;
use super::{AgentOutcome, AgentRunRequest, CheckExecutor, assemble_instructions};
use crate::checks::config::Effort;

/// The sandbox-relative filename the fallback agent writes its verdict to.
const REPORT_FILE: &str = ".multitool-check-report.json";

/// Runs each check by invoking `claude -p` non-interactively. Model/provider/
/// effort come from injected configuration (see [`crate::checks::config::Config`]),
/// never hardcoded here.
pub struct ClaudeExecutor {
    /// The model to run (a concrete ID, e.g. `claude-sonnet-4-6`).
    model: String,
    /// Optional model-provider base URL; when set, passed as `ANTHROPIC_BASE_URL`.
    provider_url: Option<String>,
    /// The effort level (logged for diagnostics; `claude -p` has no effort flag).
    effort: Effort,
    /// Per-agent wall-clock timeout; on expiry the child is killed and the check
    /// resolves as errored (no verdict).
    timeout: Duration,
    /// The CLI program to invoke (`claude`).
    program: String,
}

impl ClaudeExecutor {
    pub fn new(
        model: String,
        provider_url: Option<String>,
        effort: Effort,
        timeout: Duration,
    ) -> Self {
        Self {
            model,
            provider_url,
            effort,
            timeout,
            program: "claude".to_string(),
        }
    }
}

/// The reporting directive for the file-based fallback channel.
fn file_report_directive() -> String {
    format!(
        "Carry out the check described below. When — and only when — you have reached a conclusion, \
write your verdict as a single JSON object to the file `{REPORT_FILE}` in your current working \
directory, with this exact shape:\n\
  {{\"success\": true|false, \"evidence\": \"a short explanation\"}}\n\
Set `success` to true if the check passes, or false if it fails. Write the file EXACTLY ONCE, then \
stop. If you finish without writing `{REPORT_FILE}`, the check is treated as a FAILURE.",
    )
}

/// Read and parse the sentinel verdict file, if the agent wrote one.
fn read_report(working_dir: &Path) -> Option<CheckReport> {
    #[derive(serde::Deserialize)]
    struct Wire {
        success: bool,
        #[serde(default)]
        evidence: Option<String>,
    }
    let path = working_dir.join(REPORT_FILE);
    let contents = std::fs::read_to_string(path).ok()?;
    let wire: Wire = serde_json::from_str(&contents).ok()?;
    Some(CheckReport {
        success: wire.success,
        evidence: wire.evidence,
    })
}

#[async_trait]
impl CheckExecutor for ClaudeExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        tracing::debug!(
            check_id = req.check_id,
            model = %self.model,
            effort = ?self.effort,
            "dispatching claude -p check (fallback)",
        );

        let instructions = assemble_instructions(
            &req.check,
            &file_report_directive(),
            &req.working_dir,
            req.attempt,
        );

        let mut cmd = Command::new(&self.program);
        cmd.arg("-p")
            .arg(&instructions)
            .arg("--model")
            .arg(&self.model)
            // The sandbox is a throwaway CoW clone, so skip interactive
            // permission prompts (the agent runs non-interactively and must be
            // able to write its verdict file).
            .arg("--dangerously-skip-permissions")
            .current_dir(&req.working_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Ensure the child is reaped if we drop it on timeout.
            .kill_on_drop(true);
        if let Some(url) = &self.provider_url {
            cmd.env("ANTHROPIC_BASE_URL", url);
        }

        let child = cmd.spawn().into_diagnostic()?;
        match tokio::time::timeout(self.timeout, child.wait_with_output()).await {
            Ok(result) => {
                let output = result.into_diagnostic()?;
                let verdict = read_report(&req.working_dir);
                let error = if verdict.is_none() && !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Some(format!(
                        "claude -p exited without a verdict (exit {:?}){}",
                        output.status.code(),
                        stderr_suffix(&stderr),
                    ))
                } else {
                    None
                };
                Ok(AgentOutcome {
                    verdict,
                    stop_reason: Some(format!("exit {:?}", output.status.code())),
                    turns: 0,
                    error,
                    trace_jsonl: None,
                })
            }
            Err(_elapsed) => {
                // The wait future is dropped here; `kill_on_drop` reaps the child.
                Ok(AgentOutcome {
                    // A verdict file may have been written just before the timeout.
                    verdict: read_report(&req.working_dir),
                    stop_reason: None,
                    turns: 0,
                    error: Some(format!("agent timed out after {:?}", self.timeout)),
                    trace_jsonl: None,
                })
            }
        }
    }
}

fn stderr_suffix(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        let snippet: String = trimmed.chars().take(200).collect();
        format!(": {snippet}")
    }
}
