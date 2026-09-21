//! Capturing the allowlisted, read-only tool calls a check's agent made
//! (MULTI-1817), for the Jev decision engine's evidence-capture step:
//! `multi plan` freezes these calls, replays them in-host to checksum their
//! output, and decides unchanged checks without a model call at all.
//!
//! [`ReadOnlyTool`] is the single, programmatic allowlist — it must stay in
//! lockstep with the tools [`super::cersei::read_only_tools`] actually grants
//! the agent (see the lockstep test in that module). [`ToolCallCollector`] is
//! fed cersei's raw [`AgentEvent`]s from the executor's shared `on_event`
//! hook and pairs each `ToolStart` with its `ToolEnd` **by id**.
//!
//! ## Why not adjacency pairing
//!
//! cersei's runner (`crates/cersei-agent/src/runner.rs`) emits every
//! `ToolStart` of one assistant turn first, executes the tools concurrently
//! via `join_all`, and only then emits their `ToolEnd`s as a second batch.
//! Adjacent `ToolStart`/`ToolEnd` events are therefore unrelated in general;
//! only the shared `id` ties a start to its end. "Call order" means
//! `ToolStart` order, which is preserved regardless of how the `ToolEnd`s
//! arrive.

use std::collections::HashMap;
use std::sync::Mutex;

use cersei_agent::events::AgentEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::judge::JUDGE_TOOL;

/// The enumerated allowlist of read-only tools a check agent's calls may be
/// captured from. Exactly the tools [`super::cersei::read_only_tools`]
/// grants: `Read`, `Grep`, `Glob`. Directory listing goes through `Glob`;
/// there is no `ls` tool. Serializes to (and parses from) the exact wire
/// strings `"Read"` / `"Grep"` / `"Glob"` — MULTI-1820 persists these into
/// `.check-plan.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadOnlyTool {
    Read,
    Grep,
    Glob,
}

impl ReadOnlyTool {
    /// Map a cersei tool name (as returned by `Tool::name()`) to its
    /// allowlisted variant, or `None` when the name isn't on the allowlist —
    /// including the judge tool, which callers special-case separately so it
    /// can be dropped silently instead of at `warn`. `pub(super)` (rather than
    /// private) so [`super::cersei`]'s lockstep test can call it directly.
    pub(super) fn from_tool_name(name: &str) -> Option<Self> {
        match name {
            "Read" => Some(Self::Read),
            "Grep" => Some(Self::Grep),
            "Glob" => Some(Self::Glob),
            _ => None,
        }
    }
}

/// One captured, allowlisted, successfully-completed tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: ReadOnlyTool,
    pub input: Value,
}

/// A still-open call, recorded at `ToolStart` and awaiting its `ToolEnd`.
struct OpenCall {
    tool: ReadOnlyTool,
    input: Value,
}

#[derive(Default)]
struct Inner {
    /// Ids in `ToolStart` order, for the final ordering. Pushed at most once
    /// per id (a duplicate `ToolStart` for an already-seen id is ignored).
    order: Vec<String>,
    /// Allowlisted calls awaiting their `ToolEnd`, keyed by id.
    open: HashMap<String, OpenCall>,
    /// Calls that finished with `is_error == false`, keyed by id.
    done: HashMap<String, ToolCall>,
}

/// Accumulates one agent attempt's allowlisted tool calls from cersei's raw
/// event stream. Interior-mutable (`Mutex`) so it can be shared as an `Arc`
/// with the executor's synchronous `on_event` callback, mirroring
/// [`super::trace::TraceRecorder`]'s shape.
pub(super) struct ToolCallCollector {
    inner: Mutex<Inner>,
}

impl ToolCallCollector {
    pub(super) fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Feed one agent event. Only `ToolStart`/`ToolEnd` affect capture; every
    /// other event is ignored. Taking the whole event (rather than
    /// pre-destructured fields) keeps the executor's shared `on_event` hook
    /// down to a single delegating line.
    pub(super) fn record(&self, event: &AgentEvent) {
        match event {
            AgentEvent::ToolStart { name, id, input } => self.start(name, id, input),
            AgentEvent::ToolEnd { id, is_error, .. } => self.end(id, *is_error),
            _ => {}
        }
    }

    fn start(&self, name: &str, id: &str, input: &Value) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.open.contains_key(id) || inner.done.contains_key(id) {
            // Duplicate id: first `ToolStart` wins, matching this executor's
            // judge-tool first-call-wins convention.
            return;
        }
        let Some(tool) = ReadOnlyTool::from_tool_name(name) else {
            if name != JUDGE_TOOL {
                tracing::warn!(
                    tool = name,
                    id,
                    "dropping tool call outside the read-only allowlist",
                );
            }
            return;
        };
        inner.order.push(id.to_string());
        inner.open.insert(
            id.to_string(),
            OpenCall {
                tool,
                input: input.clone(),
            },
        );
    }

    fn end(&self, id: &str, is_error: bool) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // An id with no open, allowlisted call is ignored: either the id is
        // unknown (no matching `ToolStart` was ever seen) or its `ToolStart`
        // was already dropped as non-allowlisted.
        let Some(open) = inner.open.remove(id) else {
            return;
        };
        if !is_error {
            inner.done.insert(
                id.to_string(),
                ToolCall {
                    tool: open.tool,
                    input: open.input,
                },
            );
        }
        // An errored call is dropped: it stays out of `done`, and `finish`
        // skips any id in `order` that never made it there.
    }

    /// The captured calls, in `ToolStart` order. A `ToolStart` with no
    /// matching `ToolEnd` — the agent was cancelled or timed out mid-tool —
    /// is dropped, as is one whose `ToolEnd` reported `is_error == true`.
    pub(super) fn finish(&self) -> Vec<ToolCall> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let order = std::mem::take(&mut inner.order);
        order
            .into_iter()
            .filter_map(|id| inner.done.remove(&id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn start(name: &str, id: &str) -> AgentEvent {
        AgentEvent::ToolStart {
            name: name.to_string(),
            id: id.to_string(),
            input: json!({ "id": id }),
        }
    }

    fn end(id: &str, is_error: bool) -> AgentEvent {
        AgentEvent::ToolEnd {
            name: "ignored-by-end".to_string(),
            id: id.to_string(),
            result: "result".to_string(),
            is_error,
            duration: Duration::from_millis(1),
            compression: None,
        }
    }

    #[test]
    fn captures_allowlisted_calls_that_completed_successfully() {
        let c = ToolCallCollector::new();
        c.record(&start("Read", "t1"));
        c.record(&end("t1", false));

        let calls = c.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, ReadOnlyTool::Read);
        assert_eq!(calls[0].input, json!({ "id": "t1" }));
    }

    #[test]
    fn drops_calls_outside_the_allowlist() {
        let c = ToolCallCollector::new();
        c.record(&start("Bash", "t1"));
        c.record(&end("t1", false));
        c.record(&start("Write", "t2"));
        c.record(&end("t2", false));

        assert!(c.finish().is_empty());
    }

    #[test]
    fn excludes_the_judge_tool() {
        let c = ToolCallCollector::new();
        c.record(&start(JUDGE_TOOL, "t1"));
        c.record(&end("t1", false));

        assert!(c.finish().is_empty());
    }

    #[test]
    fn drops_errored_calls() {
        let c = ToolCallCollector::new();
        c.record(&start("Read", "t1"));
        c.record(&end("t1", true));

        assert!(c.finish().is_empty());
    }

    #[test]
    fn drops_a_tool_start_with_no_matching_tool_end() {
        let c = ToolCallCollector::new();
        c.record(&start("Read", "t1"));
        // No matching ToolEnd: the agent was cancelled or timed out mid-tool.

        assert!(c.finish().is_empty());
    }

    #[test]
    fn ignores_a_tool_end_with_an_unknown_id() {
        let c = ToolCallCollector::new();
        c.record(&end("no-such-id", false));

        assert!(c.finish().is_empty());
    }

    #[test]
    fn preserves_tool_start_order_even_when_tool_ends_arrive_batched_out_of_order() {
        // Mirrors cersei's actual emission order: every ToolStart of a turn
        // first, then the batched ToolEnds in a different (here: reversed)
        // order once the parallel tool executions resolve.
        let c = ToolCallCollector::new();
        c.record(&start("Read", "t1"));
        c.record(&start("Grep", "t2"));
        c.record(&start("Glob", "t3"));
        c.record(&end("t3", false));
        c.record(&end("t1", false));
        c.record(&end("t2", false));

        let calls = c.finish();
        assert_eq!(
            calls.iter().map(|c| c.tool).collect::<Vec<_>>(),
            vec![ReadOnlyTool::Read, ReadOnlyTool::Grep, ReadOnlyTool::Glob]
        );
    }

    #[test]
    fn a_duplicate_tool_start_id_keeps_the_first() {
        let c = ToolCallCollector::new();
        c.record(&AgentEvent::ToolStart {
            name: "Read".to_string(),
            id: "t1".to_string(),
            input: json!({ "file_path": "first.rs" }),
        });
        c.record(&AgentEvent::ToolStart {
            name: "Grep".to_string(),
            id: "t1".to_string(),
            input: json!({ "pattern": "second" }),
        });
        c.record(&end("t1", false));

        let calls = c.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, ReadOnlyTool::Read);
        assert_eq!(calls[0].input, json!({ "file_path": "first.rs" }));
    }

    #[test]
    fn a_duplicate_tool_end_id_keeps_the_first_outcome() {
        let c = ToolCallCollector::new();
        c.record(&start("Read", "t1"));
        c.record(&end("t1", false));
        // A second ToolEnd for the same id has nothing open to pair with
        // (already resolved) and must not resurrect or overwrite the call.
        c.record(&end("t1", true));

        let calls = c.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, ReadOnlyTool::Read);
    }

    #[test]
    fn non_tool_events_are_ignored() {
        let c = ToolCallCollector::new();
        c.record(&AgentEvent::TurnStart { turn: 1 });
        c.record(&AgentEvent::TextDelta("hello".to_string()));

        assert!(c.finish().is_empty());
    }

    #[test]
    fn serializes_read_only_tool_with_the_exact_wire_strings() {
        assert_eq!(
            serde_json::to_string(&ReadOnlyTool::Read).unwrap(),
            "\"Read\""
        );
        assert_eq!(
            serde_json::to_string(&ReadOnlyTool::Grep).unwrap(),
            "\"Grep\""
        );
        assert_eq!(
            serde_json::to_string(&ReadOnlyTool::Glob).unwrap(),
            "\"Glob\""
        );
        let round_tripped: ReadOnlyTool = serde_json::from_str("\"Glob\"").unwrap();
        assert_eq!(round_tripped, ReadOnlyTool::Glob);
    }
}
