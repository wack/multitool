//! The agent-executor seam (M2). [`CheckExecutor`] abstracts "run one check's
//! agent" so the concrete `claude -p` executor can later be swapped for a Claude
//! Code SDK (or other provider) without touching the execution phase. Per the
//! spec it is a boxed trait object for dynamic dispatch, mirroring the repo's
//! `BoxedIngress` / `BoxedMonitor` / `BoxedPlatform` convention.

pub mod claude;
#[cfg(test)]
mod fake;

use std::path::PathBuf;

use async_trait::async_trait;
use miette::Result;

use crate::checks::mcp::{CheckReport, REPORT_TOOL};
use crate::checks::model::{Check, CheckId};

#[cfg(test)]
pub use fake::FakeExecutor;

/// Everything an executor needs to run one check's agent.
pub struct AgentRunRequest {
    /// The check this request runs, for routing/labelling.
    pub check_id: CheckId,
    /// The assembled instructions + check prompt (see [`assemble_instructions`]).
    pub instructions: String,
    /// The sandbox directory to run the agent in (its working directory).
    pub working_dir: PathBuf,
    /// Path to the per-check `--mcp-config` JSON file (points at this check's
    /// dedicated MCP endpoint).
    pub mcp_config_path: PathBuf,
}

/// Process-level signal from running an agent.
///
/// **Note:** the authoritative verdict is the MCP-reported result, not this.
/// [`AgentOutcome::reported`] is an *optional* inline verdict for executors that
/// capture the tool call directly (a future in-process SDK, or the test fake);
/// the shell-out `claude -p` executor always leaves it `None`.
#[derive(Debug, Clone, Default)]
pub struct AgentOutcome {
    /// Whether the agent process exited with a success status.
    pub exited_cleanly: bool,
    /// The process exit code, if one was produced.
    pub exit_code: Option<i32>,
    /// Captured stderr, for surfacing execution errors (distinct from a check
    /// merely *failing*).
    pub stderr: String,
    /// An inline verdict obtained by the executor itself, if any.
    pub reported: Option<CheckReport>,
}

/// The abstraction over running a single check's agent.
#[async_trait]
pub trait CheckExecutor: Send + Sync {
    /// Run a single check's agent against its dedicated MCP endpoint.
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome>;
}

/// A boxed [`CheckExecutor`] for dynamic dispatch (DI seam).
pub type BoxedExecutor = Box<dyn CheckExecutor + Send + Sync>;

/// Assemble the instruction text handed to an agent: standing operating
/// instructions (it MUST call the report tool exactly once) plus the check
/// prompt verbatim. (MULTI-1350)
pub fn assemble_instructions(check: &Check) -> String {
    format!(
        "You are validating a single requirement for the MultiTool Checks tool.\n\
Your current working directory is a sandboxed, throwaway copy of the user's repository; \
you may read it and run commands against it freely.\n\
\n\
Carry out the check described below. When — and only when — you have reached a conclusion, \
you MUST call the `{REPORT_TOOL}` tool EXACTLY ONCE:\n\
  - set `success` to true if the check passes, or false if it fails;\n\
  - optionally set `evidence` to a short explanation of how you concluded.\n\
Report your result ONLY through `{REPORT_TOOL}` — not via stdout, not via a file — and do not \
call it more than once. After calling it, stop. If you finish without calling `{REPORT_TOOL}`, \
the check is treated as a FAILURE.\n\
\n\
--- CHECK: {title} ---\n\
{prompt}\n",
        title = check.title,
        prompt = check.prompt,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(CheckExecutor);

    #[test]
    fn instructions_embed_prompt_and_demand_single_report() {
        let check = Check {
            title: "No yellow".into(),
            prompt: "scan for yellow text".into(),
        };
        let text = assemble_instructions(&check);
        assert!(text.contains("scan for yellow text"));
        assert!(text.contains(REPORT_TOOL));
        assert!(text.contains("EXACTLY ONCE"));
        assert!(text.contains("No yellow"));
    }
}
