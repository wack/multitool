//! The concrete [`CheckExecutor`] for the MVP: shell out to the Claude Code CLI
//! via `claude -p`. The shell specifics live behind the trait so a future Claude
//! Code SDK executor can replace this without touching execution.

use std::time::Duration;

use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};
use tokio::process::Command;

use super::{AgentOutcome, AgentRunRequest, CheckExecutor};
use crate::checks::config::Effort;

/// Runs each check by invoking `claude -p` non-interactively. Model/provider/
/// effort come from injected configuration (see [`crate::checks::config::Config`]),
/// never hardcoded here.
pub struct ClaudeExecutor {
    /// The model family to run (e.g. the `sonnet` family for the MVP).
    model: String,
    /// Optional model-provider base URL; when set, passed as `ANTHROPIC_BASE_URL`.
    provider_url: Option<String>,
    /// The effort level (logged for now; see TODO in `run_check`).
    effort: Effort,
    /// Per-agent wall-clock timeout; on expiry the child is killed and the check
    /// resolves as errored (no report).
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

#[async_trait]
impl CheckExecutor for ClaudeExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        // TODO: "effort level" has no clean `claude -p` flag yet; for
        // now it is recorded for diagnostics and wired through when a richer
        // provider lands.
        tracing::debug!(
            check_id = req.check_id,
            model = %self.model,
            effort = ?self.effort,
            "dispatching claude -p check",
        );

        let mut cmd = Command::new(&self.program);
        cmd.arg("-p")
            .arg(&req.instructions)
            .arg("--model")
            .arg(&self.model)
            .arg("--mcp-config")
            .arg(&req.mcp_config_path)
            // The sandbox is a throwaway CoW clone, so skip interactive
            // permission prompts (the agent runs non-interactively).
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
                Ok(AgentOutcome {
                    exited_cleanly: output.status.success(),
                    exit_code: output.status.code(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                    reported: None, // authoritative verdict arrives via MCP
                })
            }
            Err(_elapsed) => {
                // The wait future is dropped here; `kill_on_drop` reaps the child.
                Ok(AgentOutcome {
                    exited_cleanly: false,
                    exit_code: None,
                    stderr: format!("agent timed out after {:?}", self.timeout),
                    reported: None,
                })
            }
        }
    }
}
