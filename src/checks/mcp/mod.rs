//! The in-process MCP result-reporting server — the trustworthy guardrail.
//!
//! Because agents are nondeterministic, we do **not** trust stdout or sentinel
//! files. Every agent reports its verdict by calling a single MCP tool,
//! [`REPORT_TOOL`] (`report-check-result`), served by **one** in-process `rmcp`
//! server bound to a localhost port and run on a dedicated tokio task within
//! this process (never a subprocess).
//!
//! The single server hosts **N endpoints — one per check** (`/checks/{id}`), so
//! each agent has a unique URL to write its singleton result to. Reports flow
//! back to the main task over an mpsc channel keyed by check id. Once every
//! check has reported (or a grace period elapses for missing reports), the
//! server task is cancelled and control returns to execution.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use miette::{IntoDiagnostic, Result};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::checks::model::CheckId;

/// The exact MCP tool name agents call to report a verdict. Referenced verbatim
/// by the agent instructions (M5) and the `--mcp-config` payload.
pub const REPORT_TOOL: &str = "report-check-result";

/// The MCP server name advertised in the `--mcp-config` payload.
pub const SERVER_NAME: &str = "multitool-checks";

/// The arguments of a `report-check-result` tool call (the wire contract).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReportCheckResult {
    /// The check's verdict — `true` means the requirement is satisfied.
    pub success: bool,
    /// Optional explanation of how the agent reached its conclusion.
    #[serde(default)]
    pub evidence: Option<String>,
}

/// A verdict recorded by the server for one check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub success: bool,
    pub evidence: Option<String>,
}

/// The per-check MCP handler. One is constructed per session by the service
/// factory; all sessions for a given check share the same `reported` flag and
/// result `tx`, so single-call semantics hold across reconnects.
#[derive(Clone)]
struct ReportServer {
    check_id: CheckId,
    tx: UnboundedSender<(CheckId, CheckReport)>,
    reported: Arc<AtomicBool>,
}

#[tool_router]
impl ReportServer {
    #[tool(
        name = "report-check-result",
        description = "Report whether this check passed. Call exactly once: set success=true if the check passes or false if it fails, with optional evidence explaining your reasoning."
    )]
    async fn report_check_result(
        &self,
        params: Parameters<ReportCheckResult>,
    ) -> Result<CallToolResult, ErrorData> {
        let Parameters(input) = params;
        let recorded = self.record(CheckReport {
            success: input.success,
            evidence: input.evidence,
        });
        let msg = if recorded {
            "result recorded"
        } else {
            "result already recorded for this check; ignoring duplicate"
        };
        Ok(CallToolResult::success(vec![Content::text(msg)]))
    }
}

impl ReportServer {
    /// Record the report with single-call semantics. Returns `true` if this was
    /// the first (and only honored) call for the check, `false` for a duplicate.
    fn record(&self, report: CheckReport) -> bool {
        if self.reported.swap(true, Ordering::SeqCst) {
            tracing::warn!(
                check_id = self.check_id,
                "duplicate report-check-result call ignored"
            );
            return false;
        }
        // The receiver lives for the whole run; a send error only means the run
        // is already tearing down, which we can safely ignore.
        let _ = self.tx.send((self.check_id, report));
        true
    }
}

#[tool_handler]
impl ServerHandler for ReportServer {}

/// A handle to the running result server: the bound port, the result channel,
/// and the means to shut the server task down.
pub struct ResultServer {
    base_url: String,
    cancel: CancellationToken,
    join: JoinHandle<()>,
    rx: UnboundedReceiver<(CheckId, CheckReport)>,
    /// Kept alive purely so the result channel never closes while checks run.
    _keepalive: UnboundedSender<(CheckId, CheckReport)>,
}

/// The URL path hosting the endpoint for `check_id`.
fn endpoint_path(check_id: CheckId) -> String {
    format!("/checks/{check_id}")
}

impl ResultServer {
    /// Stand up the single server with one endpoint per check id, bound to an
    /// OS-assigned localhost port, on a dedicated tokio task.
    pub async fn start(check_ids: &[CheckId]) -> Result<Self> {
        let (tx, rx) = unbounded_channel::<(CheckId, CheckReport)>();
        let cancel = CancellationToken::new();
        let session_manager = Arc::new(LocalSessionManager::default());

        let mut router = axum::Router::new();
        for &id in check_ids {
            let reported = Arc::new(AtomicBool::new(false));
            let tx_for_check = tx.clone();
            // `StreamableHttpServerConfig` is `#[non_exhaustive]`, so build it
            // from `default()` and tweak the public fields we care about.
            // One short request/response per check; stateless + plain JSON keeps
            // the single tool call simple (no SSE framing needed).
            let mut config = StreamableHttpServerConfig::default();
            config.stateful_mode = false;
            config.json_response = true;
            config.cancellation_token = cancel.clone();
            let factory = move || {
                Ok::<_, std::io::Error>(ReportServer {
                    check_id: id,
                    tx: tx_for_check.clone(),
                    reported: reported.clone(),
                })
            };
            let service = StreamableHttpService::new(factory, session_manager.clone(), config);
            router = router.nest_service(&endpoint_path(id), service);
        }

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .into_diagnostic()?;
        let port = listener.local_addr().into_diagnostic()?.port();

        let shutdown = cancel.clone();
        let join = tokio::spawn(async move {
            let server = axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled().await });
            if let Err(e) = server.await {
                tracing::error!("MCP result server error: {e}");
            }
        });

        Ok(Self {
            base_url: format!("http://127.0.0.1:{port}"),
            cancel,
            join,
            rx,
            _keepalive: tx,
        })
    }

    /// The full endpoint URL an agent should connect to for `check_id`.
    pub fn endpoint_url(&self, check_id: CheckId) -> String {
        format!("{}{}", self.base_url, endpoint_path(check_id))
    }

    /// Collect reports until every id in `needed` has reported or `grace`
    /// elapses. Available reports are drained immediately; only genuinely
    /// missing ones incur waiting. Missing ids are simply absent from the map
    /// (reconciliation treats them as failures).
    pub async fn collect(
        &mut self,
        needed: &HashSet<CheckId>,
        grace: Duration,
    ) -> HashMap<CheckId, CheckReport> {
        let mut map = HashMap::new();
        while let Ok((id, report)) = self.rx.try_recv() {
            map.entry(id).or_insert(report);
        }
        let have_all =
            |m: &HashMap<CheckId, CheckReport>| needed.iter().all(|id| m.contains_key(id));
        if have_all(&map) {
            return map;
        }
        let deadline = tokio::time::Instant::now() + grace;
        while !have_all(&map) {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some((id, report))) => {
                    map.entry(id).or_insert(report);
                }
                Ok(None) => break, // all senders dropped (shouldn't happen: keepalive)
                Err(_) => break,   // grace elapsed
            }
        }
        map
    }

    /// Signal the server task to stop and wait for it to wind down.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.join.await;
    }
}

/// Build the `--mcp-config` JSON payload pointing an agent at `endpoint_url`,
/// declaring the `report-check-result` server under [`SERVER_NAME`]. (M4 #1348)
pub fn mcp_config_json(endpoint_url: &str) -> String {
    let server = serde_json::json!({ "type": "http", "url": endpoint_url });
    let mut servers = serde_json::Map::new();
    servers.insert(SERVER_NAME.to_string(), server);
    let mut root = serde_json::Map::new();
    root.insert("mcpServers".to_string(), serde_json::Value::Object(servers));
    serde_json::Value::Object(root).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_enforces_single_call_and_delivers() {
        let (tx, mut rx) = unbounded_channel();
        let server = ReportServer {
            check_id: 7,
            tx,
            reported: Arc::new(AtomicBool::new(false)),
        };

        assert!(server.record(CheckReport {
            success: true,
            evidence: Some("ok".into())
        }));
        // Duplicate is ignored.
        assert!(!server.record(CheckReport {
            success: false,
            evidence: None
        }));

        let (id, report) = rx.try_recv().expect("first report delivered");
        assert_eq!(id, 7);
        assert!(report.success);
        assert_eq!(report.evidence.as_deref(), Some("ok"));
        // No second message was sent.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn mcp_config_targets_the_endpoint_and_tool_server() {
        let json = mcp_config_json("http://127.0.0.1:5050/checks/3");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value["mcpServers"]["multitool-checks"]["url"],
            "http://127.0.0.1:5050/checks/3"
        );
        assert_eq!(value["mcpServers"]["multitool-checks"]["type"], "http");
    }

    #[tokio::test]
    async fn server_binds_a_port_and_shuts_down() {
        let mut server = ResultServer::start(&[0, 1, 2]).await.unwrap();
        assert!(server.endpoint_url(1).ends_with("/checks/1"));
        assert!(server.endpoint_url(1).starts_with("http://127.0.0.1:"));
        // No reports arrive; collect returns promptly after the grace window.
        let needed: HashSet<CheckId> = [0usize, 1, 2].into_iter().collect();
        let got = server.collect(&needed, Duration::from_millis(50)).await;
        assert!(got.is_empty());
        server.shutdown().await;
    }
}
