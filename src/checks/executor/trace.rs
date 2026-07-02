//! Opt-in session-trace capture for the in-process cersei executor.
//!
//! When `multi check --trace-archive` is set, each check execution attaches a
//! [`TraceRecorder`] as the agent's `on_event` sink. The recorder coalesces the
//! streamed [`AgentEvent`]s into a compact ordered list of [`TraceRecord`]s
//! (token-level text/thinking deltas are joined into per-turn blocks), and once
//! the run resolves [`serialize_trace`] renders them as a self-contained NDJSON
//! document: a `header` line, the records, then an `outcome` footer. Those bytes
//! travel back in [`AgentOutcome::trace_jsonl`] and are bundled by
//! [`crate::checks::trace_archive`].
//!
//! ## Why `on_event`, not cersei's `.memory()` session persistence
//!
//! cersei can persist a Claude-Code-compatible transcript via a `Memory`
//! backend, but it flushes `memory.store()` only on the agentic loop's *normal*
//! exit. This executor cancels its agent the instant a verdict lands and wraps
//! the run in a drop-on-timeout, so the store call is skipped for exactly the
//! cancelled / timed-out / errored runs this feature exists to debug. `emit()`
//! fires the `on_event` handler for every event *before* those early returns, so
//! an event-sourced trace survives them. (Two events — `ModelRequestStart` /
//! `ModelResponseStart` — are emitted only on the streaming channel, which the
//! blocking `run()` path drops, so they never reach us; everything else does.)

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use cersei_agent::events::AgentEvent;
use cersei_types::{StopReason, Usage};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use super::AgentOutcome;
use crate::checks::config::Effort;

/// Cap on a single captured tool result. File reads and greps can be enormous;
/// past this we truncate (with a marker) to keep the archive bounded.
const MAX_TOOL_RESULT_BYTES: usize = 16 * 1024;

/// The on-disk trace format version, stamped in the header so a reader can adapt.
const TRACE_FORMAT_VERSION: u32 = 1;

/// A recorder attached to one agent run via `Agent::on_event`. Interior-mutable
/// so it can live behind an `Arc` shared with the (synchronous) event callback.
pub(super) struct TraceRecorder {
    inner: Mutex<Coalescer>,
    /// Monotonic start, for the relative `elapsed_ms` stamp on each record.
    started: Instant,
    /// Wall-clock start, recorded in the trace header.
    started_at: DateTime<Utc>,
}

impl TraceRecorder {
    pub(super) fn new() -> Self {
        Self {
            inner: Mutex::new(Coalescer::default()),
            started: Instant::now(),
            started_at: Utc::now(),
        }
    }

    /// Record one event. Called synchronously from `Agent::emit` for every event
    /// on the run, including on the cancel/timeout/error paths.
    pub(super) fn record(&self, event: &AgentEvent) {
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        // A poisoned lock only means a prior panic while recording; recover the
        // guard and keep capturing rather than panicking the agent's event path.
        let mut c = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            AgentEvent::TextDelta(t) => {
                c.flush_thinking(elapsed_ms);
                c.pending_text.push_str(t);
            }
            AgentEvent::ThinkingDelta(t) => {
                c.flush_text(elapsed_ms);
                c.pending_thinking.push_str(t);
            }
            AgentEvent::ToolStart { name, id, input } => c.push(
                elapsed_ms,
                TraceRecord::ToolStart {
                    name: name.clone(),
                    id: id.clone(),
                    input: input.clone(),
                },
            ),
            AgentEvent::ToolEnd {
                name,
                id,
                result,
                is_error,
                duration,
                ..
            } => c.push(
                elapsed_ms,
                TraceRecord::ToolEnd {
                    name: name.clone(),
                    id: id.clone(),
                    is_error: *is_error,
                    duration_ms: duration.as_millis() as u64,
                    result: truncate_result(result),
                },
            ),
            AgentEvent::ToolPermissionCheck { name, id, level } => c.push(
                elapsed_ms,
                TraceRecord::ToolPermissionCheck {
                    name: name.clone(),
                    id: id.clone(),
                    level: format!("{level:?}"),
                },
            ),
            AgentEvent::PermissionRequired(req) => c.push(
                elapsed_ms,
                TraceRecord::PermissionRequired {
                    detail: format!("{req:?}"),
                },
            ),
            AgentEvent::TurnStart { turn } => {
                c.push(elapsed_ms, TraceRecord::TurnStart { turn: *turn })
            }
            AgentEvent::TurnComplete {
                turn,
                stop_reason,
                usage,
            } => c.push(
                elapsed_ms,
                TraceRecord::TurnComplete {
                    turn: *turn,
                    stop_reason: stop_reason.clone(),
                    usage: usage.clone(),
                },
            ),
            // Emitted only on the streaming channel, which the blocking `run()`
            // path drops — recorded here for completeness should that change.
            AgentEvent::ModelRequestStart {
                turn,
                message_count,
                token_estimate,
            } => c.push(
                elapsed_ms,
                TraceRecord::ModelRequestStart {
                    turn: *turn,
                    message_count: *message_count,
                    token_estimate: *token_estimate,
                },
            ),
            AgentEvent::ModelResponseStart { turn, model } => c.push(
                elapsed_ms,
                TraceRecord::ModelResponseStart {
                    turn: *turn,
                    model: model.clone(),
                },
            ),
            AgentEvent::TokenWarning { pct_used, state } => c.push(
                elapsed_ms,
                TraceRecord::TokenWarning {
                    pct_used: *pct_used,
                    state: format!("{state:?}"),
                },
            ),
            AgentEvent::CompactStart {
                reason,
                messages_before,
            } => c.push(
                elapsed_ms,
                TraceRecord::CompactStart {
                    reason: format!("{reason:?}"),
                    messages_before: *messages_before,
                },
            ),
            AgentEvent::CompactEnd {
                messages_after,
                tokens_freed,
            } => c.push(
                elapsed_ms,
                TraceRecord::CompactEnd {
                    messages_after: *messages_after,
                    tokens_freed: *tokens_freed,
                },
            ),
            AgentEvent::SessionLoaded {
                session_id,
                message_count,
            } => c.push(
                elapsed_ms,
                TraceRecord::SessionLoaded {
                    session_id: session_id.clone(),
                    message_count: *message_count,
                },
            ),
            AgentEvent::SessionSaved { session_id } => c.push(
                elapsed_ms,
                TraceRecord::SessionSaved {
                    session_id: session_id.clone(),
                },
            ),
            AgentEvent::CostUpdate {
                turn_cost,
                cumulative_cost,
                input_tokens,
                output_tokens,
            } => c.push(
                elapsed_ms,
                TraceRecord::CostUpdate {
                    turn_cost: *turn_cost,
                    cumulative_cost: *cumulative_cost,
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                },
            ),
            AgentEvent::SubAgentSpawned { agent_id, prompt } => c.push(
                elapsed_ms,
                TraceRecord::SubAgentSpawned {
                    agent_id: agent_id.clone(),
                    prompt: prompt.clone(),
                },
            ),
            AgentEvent::SubAgentComplete { agent_id, result } => c.push(
                elapsed_ms,
                TraceRecord::SubAgentComplete {
                    agent_id: agent_id.clone(),
                    turns: result.turns,
                },
            ),
            AgentEvent::HookFired { event, hook_name } => c.push(
                elapsed_ms,
                TraceRecord::HookFired {
                    hook_event: format!("{event:?}"),
                    hook_name: hook_name.clone(),
                },
            ),
            AgentEvent::HookBlocked {
                event,
                hook_name,
                reason,
            } => c.push(
                elapsed_ms,
                TraceRecord::HookBlocked {
                    hook_event: format!("{event:?}"),
                    hook_name: hook_name.clone(),
                    reason: reason.clone(),
                },
            ),
            AgentEvent::Status(s) => c.push(elapsed_ms, TraceRecord::Status { message: s.clone() }),
            AgentEvent::Error(s) => c.push(elapsed_ms, TraceRecord::Error { message: s.clone() }),
            AgentEvent::Complete(output) => c.push(
                elapsed_ms,
                TraceRecord::Complete {
                    turns: output.turns,
                    stop_reason: output.stop_reason.clone(),
                },
            ),
        }
    }

    /// Drain the coalesced records, flushing any pending delta buffers first.
    fn drain(&self) -> Vec<StampedRecord> {
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        let mut c = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        c.flush_deltas(elapsed_ms);
        std::mem::take(&mut c.records)
    }
}

/// Accumulates records, coalescing contiguous runs of text/thinking deltas.
#[derive(Default)]
struct Coalescer {
    records: Vec<StampedRecord>,
    pending_text: String,
    pending_thinking: String,
}

impl Coalescer {
    fn flush_thinking(&mut self, elapsed_ms: u64) {
        if !self.pending_thinking.is_empty() {
            let text = std::mem::take(&mut self.pending_thinking);
            self.records.push(StampedRecord {
                elapsed_ms,
                record: TraceRecord::Thinking { text },
            });
        }
    }

    fn flush_text(&mut self, elapsed_ms: u64) {
        if !self.pending_text.is_empty() {
            let text = std::mem::take(&mut self.pending_text);
            self.records.push(StampedRecord {
                elapsed_ms,
                record: TraceRecord::Text { text },
            });
        }
    }

    fn flush_deltas(&mut self, elapsed_ms: u64) {
        self.flush_thinking(elapsed_ms);
        self.flush_text(elapsed_ms);
    }

    /// Flush any pending deltas (to preserve ordering), then push `record`.
    fn push(&mut self, elapsed_ms: u64, record: TraceRecord) {
        self.flush_deltas(elapsed_ms);
        self.records.push(StampedRecord { elapsed_ms, record });
    }
}

/// A record plus the milliseconds since the run started.
struct StampedRecord {
    elapsed_ms: u64,
    record: TraceRecord,
}

impl StampedRecord {
    /// Render as a single NDJSON object, splicing `elapsed_ms` alongside the
    /// record's own fields. Built via `to_value` + insert rather than
    /// `#[serde(flatten)]` to sidestep flatten's internally-tagged-enum quirks.
    fn to_json_line(&self) -> String {
        let mut value = serde_json::to_value(&self.record).unwrap_or_else(
            |e| json!({ "event": "trace_serialize_error", "detail": e.to_string() }),
        );
        if let Value::Object(map) = &mut value {
            map.insert("elapsed_ms".to_string(), Value::from(self.elapsed_ms));
        }
        value.to_string()
    }
}

/// One coalesced moment in the agent session. Serialized as an internally-tagged
/// object (`{"event": "...", ...}`); non-serializable cersei payloads (permission
/// requests, hook events, warning/compaction enums) are captured as `Debug`
/// strings, which is plenty for post-hoc debugging.
#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum TraceRecord {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolStart {
        name: String,
        id: String,
        input: Value,
    },
    ToolEnd {
        name: String,
        id: String,
        is_error: bool,
        duration_ms: u64,
        result: String,
    },
    ToolPermissionCheck {
        name: String,
        id: String,
        level: String,
    },
    PermissionRequired {
        detail: String,
    },
    TurnStart {
        turn: u32,
    },
    TurnComplete {
        turn: u32,
        stop_reason: StopReason,
        usage: Usage,
    },
    ModelRequestStart {
        turn: u32,
        message_count: usize,
        token_estimate: u64,
    },
    ModelResponseStart {
        turn: u32,
        model: String,
    },
    TokenWarning {
        pct_used: f64,
        state: String,
    },
    CompactStart {
        reason: String,
        messages_before: usize,
    },
    CompactEnd {
        messages_after: usize,
        tokens_freed: u64,
    },
    SessionLoaded {
        session_id: String,
        message_count: usize,
    },
    SessionSaved {
        session_id: String,
    },
    CostUpdate {
        turn_cost: f64,
        cumulative_cost: f64,
        input_tokens: u64,
        output_tokens: u64,
    },
    SubAgentSpawned {
        agent_id: String,
        prompt: String,
    },
    SubAgentComplete {
        agent_id: String,
        turns: u32,
    },
    HookFired {
        hook_event: String,
        hook_name: String,
    },
    HookBlocked {
        hook_event: String,
        hook_name: String,
        reason: String,
    },
    Status {
        message: String,
    },
    Error {
        message: String,
    },
    Complete {
        turns: u32,
        stop_reason: StopReason,
    },
}

/// Truncate a tool result to [`MAX_TOOL_RESULT_BYTES`] on a char boundary,
/// appending a marker noting how many bytes were dropped.
fn truncate_result(s: &str) -> String {
    if s.len() <= MAX_TOOL_RESULT_BYTES {
        return s.to_string();
    }
    let mut end = MAX_TOOL_RESULT_BYTES;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated {} bytes]", &s[..end], s.len() - end)
}

/// Fields the executor knows about the execution, stamped into the trace header.
/// (Requirement grouping and the retry/attempt number are added by the archive
/// layer as the file's path, since the executor doesn't know them.)
pub(super) struct TraceHeader<'a> {
    pub check_id: usize,
    pub check_title: &'a str,
    pub model: &'a str,
    pub effort: Effort,
    pub working_dir: &'a Path,
    pub session_id: &'a str,
}

/// Render the recorded session as a self-contained NDJSON document: a `header`
/// line, one line per coalesced record, then an `outcome` footer summarizing the
/// authoritative [`AgentOutcome`] (which is meaningful even when no terminal
/// `Complete`/`Error` event arrived, e.g. on a drop-on-timeout).
pub(super) fn serialize_trace(
    recorder: &TraceRecorder,
    header: &TraceHeader<'_>,
    outcome: &AgentOutcome,
) -> Vec<u8> {
    let records = recorder.drain();

    let mut out = String::new();

    let header_line = json!({
        "event": "header",
        "trace_format": TRACE_FORMAT_VERSION,
        "check_id": header.check_id,
        "check_title": header.check_title,
        "model": header.model,
        "effort": format!("{:?}", header.effort).to_lowercase(),
        "working_dir": header.working_dir.display().to_string(),
        "session_id": header.session_id,
        "started_at": recorder.started_at.to_rfc3339(),
    });
    out.push_str(&header_line.to_string());
    out.push('\n');

    for rec in &records {
        out.push_str(&rec.to_json_line());
        out.push('\n');
    }

    let verdict = outcome
        .verdict
        .as_ref()
        .map(|v| json!({ "success": v.success, "evidence": v.evidence }));
    let footer_line = json!({
        "event": "outcome",
        "verdict": verdict,
        "stop_reason": outcome.stop_reason,
        "turns": outcome.turns,
        "error": outcome.error,
        "finished_at": Utc::now().to_rfc3339(),
    });
    out.push_str(&footer_line.to_string());
    out.push('\n');

    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::checks::executor::CheckReport;

    fn parse_lines(bytes: &[u8]) -> Vec<Value> {
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .collect()
    }

    #[test]
    fn coalesces_deltas_and_frames_header_and_outcome() {
        let rec = TraceRecorder::new();
        // Two contiguous text deltas must coalesce into one record.
        rec.record(&AgentEvent::TextDelta("Hello ".into()));
        rec.record(&AgentEvent::TextDelta("world".into()));
        rec.record(&AgentEvent::ToolStart {
            name: "grep_tool".into(),
            id: "t1".into(),
            input: json!({ "pattern": "yellow" }),
        });
        rec.record(&AgentEvent::ToolEnd {
            name: "grep_tool".into(),
            id: "t1".into(),
            result: "no matches".into(),
            is_error: false,
            duration: Duration::from_millis(5),
            compression: None,
        });
        rec.record(&AgentEvent::TurnComplete {
            turn: 1,
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        });

        let outcome = AgentOutcome {
            verdict: Some(CheckReport {
                success: true,
                evidence: Some("ok".into()),
            }),
            stop_reason: Some("EndTurn".into()),
            turns: 1,
            error: None,
            trace_jsonl: None,
        };
        let header = TraceHeader {
            check_id: 3,
            check_title: "No yellow",
            model: "claude-sonnet-4-6",
            effort: Effort::Low,
            working_dir: Path::new("/tmp/sandbox"),
            session_id: "multi-check-3",
        };

        let events = parse_lines(&serialize_trace(&rec, &header, &outcome));

        // First line is the header, last is the authoritative outcome.
        assert_eq!(events.first().unwrap()["event"], "header");
        assert_eq!(events.first().unwrap()["check_id"], 3);
        assert_eq!(events.first().unwrap()["effort"], "low");
        let last = events.last().unwrap();
        assert_eq!(last["event"], "outcome");
        assert_eq!(last["verdict"]["success"], true);
        assert_eq!(last["turns"], 1);

        // The two text deltas became a single coalesced record.
        let texts: Vec<_> = events.iter().filter(|e| e["event"] == "text").collect();
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0]["text"], "Hello world");

        assert!(events.iter().any(|e| e["event"] == "tool_start"));
        assert!(
            events
                .iter()
                .any(|e| e["event"] == "tool_end" && e["result"] == "no matches")
        );
        // Every record line (between the header and outcome frames) carries a
        // relative timestamp; the frames themselves don't.
        assert!(
            events
                .iter()
                .filter(|e| e["event"] != "header" && e["event"] != "outcome")
                .all(|e| e.get("elapsed_ms").is_some())
        );
    }

    #[test]
    fn truncates_oversized_tool_results() {
        let rec = TraceRecorder::new();
        rec.record(&AgentEvent::ToolEnd {
            name: "file_read".into(),
            id: "t1".into(),
            result: "x".repeat(MAX_TOOL_RESULT_BYTES + 100),
            is_error: false,
            duration: Duration::from_millis(1),
            compression: None,
        });
        let outcome = AgentOutcome::default();
        let header = TraceHeader {
            check_id: 0,
            check_title: "t",
            model: "m",
            effort: Effort::Low,
            working_dir: Path::new("."),
            session_id: "s",
        };

        let events = parse_lines(&serialize_trace(&rec, &header, &outcome));
        let tool_end = events.iter().find(|e| e["event"] == "tool_end").unwrap();
        let result = tool_end["result"].as_str().unwrap();
        assert!(result.contains("truncated"), "expected truncation marker");
        // Bounded to the cap plus the short marker.
        assert!(result.len() <= MAX_TOOL_RESULT_BYTES + 40);
    }
}
