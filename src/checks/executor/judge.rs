//! The per-check "judge" tool — the in-process verdict sink that replaces the
//! MCP result server (MULTI-1367).
//!
//! Agents are nondeterministic, so we do **not** trust their stdout or any
//! sentinel file: every agent reports its verdict by calling exactly one tool,
//! [`JUDGE_TOOL`]. A fresh [`JudgeTool`] is built **per check**, closing over its
//! own [`VerdictSink`] and the agent's [`CancellationToken`]. When the agent
//! calls it, the verdict is recorded into the slot (first call wins) and the
//! agent is cancelled — its job is done — so `Agent::run` returns promptly
//! instead of burning further turns. The executor reads the slot after the run.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cersei_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// The exact tool name an agent calls to report a verdict. Referenced verbatim
/// by the agent instructions (see [`super::assemble_instructions`]). Hyphens are
/// valid in Anthropic/OpenAI tool names; the name is unchanged from the MCP-era
/// `report-check-result` server so existing prompts and docs still read true.
pub const JUDGE_TOOL: &str = "report-check-result";

/// A verdict recorded for one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    /// The check's verdict — `true` means the requirement is satisfied.
    pub success: bool,
    /// Optional explanation of how the agent reached its conclusion.
    pub evidence: Option<String>,
}

/// A write-once sink for one check's verdict, shared between the [`JudgeTool`]
/// handed to the agent and the executor that reads it after the run completes.
#[derive(Clone, Default)]
pub struct VerdictSink(Arc<Mutex<Option<CheckReport>>>);

impl VerdictSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the verdict with first-call-wins semantics. Returns `true` if this
    /// was the first (honored) call, `false` for an ignored duplicate.
    fn record(&self, report: CheckReport) -> bool {
        let mut slot = self.0.lock().unwrap();
        if slot.is_some() {
            return false;
        }
        *slot = Some(report);
        true
    }

    /// The recorded verdict, if the agent reported one.
    pub fn verdict(&self) -> Option<CheckReport> {
        self.0.lock().unwrap().clone()
    }
}

/// The arguments of a `report-check-result` call (the wire contract).
#[derive(Debug, Deserialize)]
struct ReportInput {
    /// `true` if the check passes, `false` if it fails.
    success: bool,
    /// Optional short explanation of how the agent concluded.
    #[serde(default)]
    evidence: Option<String>,
}

/// The per-check judge tool. Built fresh per check, closing over its own
/// [`VerdictSink`] and the agent's [`CancellationToken`].
pub struct JudgeTool {
    sink: VerdictSink,
    cancel: CancellationToken,
}

impl JudgeTool {
    pub fn new(sink: VerdictSink, cancel: CancellationToken) -> Self {
        Self { sink, cancel }
    }
}

#[async_trait]
impl Tool for JudgeTool {
    fn name(&self) -> &str {
        JUDGE_TOOL
    }

    fn description(&self) -> &str {
        "Report whether this check passed. Call exactly once: set success=true if the check passes or false if it fails, with optional evidence explaining your reasoning."
    }

    /// Always permitted, even under a read-only policy: reporting the verdict is
    /// the agent's entire purpose and mutates nothing on disk.
    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::None
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Custom
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "success": {
                    "type": "boolean",
                    "description": "true if the check passes, false if it fails"
                },
                "evidence": {
                    "type": "string",
                    "description": "Optional short explanation of how you concluded"
                }
            },
            "required": ["success"]
        })
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> ToolResult {
        let parsed: ReportInput = match serde_json::from_value(input) {
            Ok(p) => p,
            Err(e) => return ToolResult::error(format!("invalid report arguments: {e}")),
        };
        let recorded = self.sink.record(CheckReport {
            success: parsed.success,
            evidence: parsed.evidence,
        });
        if recorded {
            // The agent's job is done; stop it after the current turn rather than
            // letting it burn the remaining turn budget.
            self.cancel.cancel();
            ToolResult::success("result recorded")
        } else {
            ToolResult::success("result already recorded for this check; ignoring duplicate")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_records_first_call_and_ignores_duplicates() {
        let sink = VerdictSink::new();
        assert!(sink.verdict().is_none());

        assert!(sink.record(CheckReport {
            success: true,
            evidence: Some("ok".into()),
        }));
        // A second call is ignored and does not overwrite.
        assert!(!sink.record(CheckReport {
            success: false,
            evidence: None,
        }));

        let stored = sink.verdict().expect("recorded");
        assert!(stored.success);
        assert_eq!(stored.evidence.as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn execute_records_verdict_and_cancels_agent() {
        let sink = VerdictSink::new();
        let cancel = CancellationToken::new();
        let tool = JudgeTool::new(sink.clone(), cancel.clone());
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("."),
            session_id: "test".into(),
            permissions: Arc::new(cersei_tools::permissions::AllowReadOnly),
            cost_tracker: Arc::new(cersei_tools::CostTracker::new()),
            mcp_manager: None,
            extensions: cersei_tools::Extensions::default(),
        };

        assert!(!cancel.is_cancelled());
        let result = tool
            .execute(json!({ "success": false, "evidence": "nope" }), &ctx)
            .await;
        assert!(!result.is_error);
        assert!(cancel.is_cancelled(), "reporting must cancel the agent");

        let verdict = sink.verdict().expect("verdict recorded");
        assert!(!verdict.success);
        assert_eq!(verdict.evidence.as_deref(), Some("nope"));

        // A duplicate call is acknowledged but does not overwrite.
        let dup = tool.execute(json!({ "success": true }), &ctx).await;
        assert!(!dup.is_error);
        assert!(!sink.verdict().unwrap().success);
    }

    #[tokio::test]
    async fn execute_rejects_malformed_input() {
        let tool = JudgeTool::new(VerdictSink::new(), CancellationToken::new());
        let ctx = ToolContext {
            working_dir: std::path::PathBuf::from("."),
            session_id: "test".into(),
            permissions: Arc::new(cersei_tools::permissions::AllowReadOnly),
            cost_tracker: Arc::new(cersei_tools::CostTracker::new()),
            mcp_manager: None,
            extensions: cersei_tools::Extensions::default(),
        };
        // `success` is required.
        let result = tool.execute(json!({ "evidence": "x" }), &ctx).await;
        assert!(result.is_error);
    }
}
