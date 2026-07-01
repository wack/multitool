//! The agent-executor seam (M2, narrowed in MULTI-1367). [`CheckExecutor`]
//! abstracts "run one check's agent → verdict/outcome". cersei-agent absorbs the
//! *provider-abstraction* rationale the seam originally carried, but not its
//! *test-seam* rationale: `cersei_agent::Agent` is a concrete struct, so the
//! execution-phase tests still need a fake. The trait keeps one method with three
//! impls — the real in-process [`cersei::CerseiExecutor`], the soon-to-retire
//! shell-out [`claude::ClaudeExecutor`] fallback (selectable for migration), and
//! the test [`FakeExecutor`]. It is a boxed trait object for dynamic dispatch,
//! mirroring the repo's `BoxedIngress` / `BoxedMonitor` / `BoxedPlatform`
//! convention.

pub mod cersei;
pub mod claude;
#[cfg(test)]
mod fake;
pub mod judge;
mod trace;

use std::path::PathBuf;

use async_trait::async_trait;
use miette::Result;

pub use judge::{CheckReport, JUDGE_TOOL};

use crate::checks::model::{Check, CheckId};

#[cfg(test)]
pub use fake::FakeExecutor;

/// Everything an executor needs to run one check's agent.
pub struct AgentRunRequest {
    /// The check this request runs, for routing/labelling and assembling the
    /// agent's instructions.
    pub check_id: CheckId,
    /// The check to validate (title + prompt). Each executor assembles its own
    /// instructions from this so it can describe its own reporting channel.
    pub check: Check,
    /// The sandbox directory to run the agent in (its working directory).
    pub working_dir: PathBuf,
}

/// The result of running one check's agent in-process.
///
/// The authoritative signal is [`AgentOutcome::verdict`]: when present, the agent
/// reported via the judge tool. The remaining fields are diagnostics that
/// distinguish the *new* failure modes — an agent that hit `max_turns` without
/// reporting, a stream error, or a timeout — so execution can synthesize a clear
/// "errored" reason when no verdict arrived.
#[derive(Debug, Clone, Default)]
pub struct AgentOutcome {
    /// The verdict the agent reported via the judge tool, if it reported at all.
    pub verdict: Option<CheckReport>,
    /// Why the agent's loop stopped (human-readable), for diagnostics when no
    /// verdict was reported.
    pub stop_reason: Option<String>,
    /// How many turns the agent took (best-effort; `0` when unavailable).
    pub turns: u32,
    /// An execution-level error distinct from a check merely *failing* (stream
    /// error, agent-build error, or timeout). `None` on a clean finish.
    pub error: Option<String>,
    /// The self-contained NDJSON session trace for this one execution, when
    /// trace capture is enabled (`multi check --trace-archive`). `None` when
    /// capture is off and for executors that don't produce traces (the `claude`
    /// fallback and the test fake). The execution layer moves these into the
    /// per-run [`crate::checks::trace_archive`] bundle.
    pub trace_jsonl: Option<Vec<u8>>,
}

impl AgentOutcome {
    /// Whether the agent reported a verdict (the only authoritative signal).
    pub fn has_verdict(&self) -> bool {
        self.verdict.is_some()
    }
}

/// The abstraction over running a single check's agent.
#[async_trait]
pub trait CheckExecutor: Send + Sync {
    /// Run a single check's agent and return its verdict/outcome.
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome>;
}

/// A boxed [`CheckExecutor`] for dynamic dispatch (DI seam).
pub type BoxedExecutor = Box<dyn CheckExecutor + Send + Sync>;

/// The reporting directive for the default (in-process) executor: call the judge
/// tool exactly once. Kept separate from [`assemble_instructions`] so the legacy
/// shell-out fallback can substitute its own reporting channel.
pub fn judge_tool_directive() -> String {
    format!(
        "Carry out the check described below. When — and only when — you have reached a conclusion, \
you MUST call the `{JUDGE_TOOL}` tool EXACTLY ONCE:\n\
  - set `success` to true if the check passes, or false if it fails;\n\
  - optionally set `evidence` to a short explanation of how you concluded.\n\
Report your result ONLY through `{JUDGE_TOOL}` — not via stdout, not via a file — and do not \
call it more than once. After calling it, stop. If you finish without calling `{JUDGE_TOOL}`, \
the check is treated as a FAILURE.",
    )
}

/// Assemble the instruction text handed to an agent: standing operating
/// instructions, the executor-supplied `reporting` directive, then the check
/// prompt verbatim. (MULTI-1350, parametrized in MULTI-1367.)
pub fn assemble_instructions(check: &Check, reporting: &str) -> String {
    format!(
        "You are validating a single requirement for the MultiTool Checks tool.\n\
Your current working directory is a sandboxed, throwaway copy of the user's repository; \
you may read it and run commands against it freely.\n\
\n\
{reporting}\n\
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
        let text = assemble_instructions(&check, &judge_tool_directive());
        assert!(text.contains("scan for yellow text"));
        assert!(text.contains(JUDGE_TOOL));
        assert!(text.contains("EXACTLY ONCE"));
        assert!(text.contains("No yellow"));
    }
}
