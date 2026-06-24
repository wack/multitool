//! The in-process MCP result-reporting server — the trustworthy guardrail.
//!
//! Because agents are nondeterministic, we do **not** trust stdout or sentinel
//! files. Every agent reports its verdict by calling a single MCP tool,
//! [`REPORT_TOOL`] (`report-check-result`), served by **one** in-process `rmcp`
//! server bound to a localhost port and run on a dedicated tokio task within
//! this process (never a subprocess).
//!
//! The single server hosts **N endpoints — one per check** (`/checks/{id}`), so
//! each agent has a unique URL to write its singleton result to. Each check has
//! a [`tokio::sync::Notify`] and a slot in a shared map; when an agent reports,
//! the handler records the verdict and notifies, so execution can wake the
//! instant a check reports (and kill that agent — its job is done).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use miette::{IntoDiagnostic, Result};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::checks::model::CheckId;

/// The exact MCP tool name agents call to report a verdict. Referenced verbatim
/// by the agent instructions (M5) and the `--mcp-config` payload.
pub const REPORT_TOOL: &str = "report-check-result";

/// The MCP server name advertised in the `--mcp-config` payload.
pub const SERVER_NAME: &str = "multitool-checks";

/// Shared store of reported verdicts, keyed by check id.
pub type ReportStore = Arc<Mutex<HashMap<CheckId, CheckReport>>>;

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
/// factory; all sessions for a given check share the same `reported` flag,
/// result store, and notifier, so single-call semantics hold across reconnects
/// and the report is observable to execution.
#[derive(Clone)]
struct ReportServer {
    check_id: CheckId,
    reported: Arc<AtomicBool>,
    reports: ReportStore,
    notify: Arc<Notify>,
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
        tracing::debug!(
            check_id = self.check_id,
            success = input.success,
            "report-check-result received"
        );
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
        self.reports.lock().unwrap().insert(self.check_id, report);
        // `notify_one` stores a permit if no one is waiting yet, so a waiter that
        // arrives after the report still wakes immediately (no lost wakeups).
        self.notify.notify_one();
        true
    }
}

#[tool_handler]
impl ServerHandler for ReportServer {}

/// A handle to the running result server: the bound port, the per-check result
/// store + notifiers, and the means to shut the server task down.
pub struct ResultServer {
    base_url: String,
    cancel: CancellationToken,
    join: JoinHandle<()>,
    reports: ReportStore,
    notifiers: HashMap<CheckId, Arc<Notify>>,
}

/// The URL path hosting the endpoint for `check_id`.
fn endpoint_path(check_id: CheckId) -> String {
    format!("/checks/{check_id}")
}

impl ResultServer {
    /// Stand up the single server with one endpoint per check id, bound to an
    /// OS-assigned localhost port, on a dedicated tokio task.
    pub async fn start(check_ids: &[CheckId]) -> Result<Self> {
        let cancel = CancellationToken::new();
        let session_manager = Arc::new(LocalSessionManager::default());
        let reports: ReportStore = Arc::new(Mutex::new(HashMap::new()));
        let mut notifiers: HashMap<CheckId, Arc<Notify>> = HashMap::new();

        let mut router = axum::Router::new();
        for &id in check_ids {
            let notify = Arc::new(Notify::new());
            notifiers.insert(id, notify.clone());
            let reported = Arc::new(AtomicBool::new(false));
            let reports_for_check = reports.clone();
            // `StreamableHttpServerConfig` is `#[non_exhaustive]`, so build it
            // from `default()` and override the fields we care about. We run in
            // *stateful* Streamable HTTP mode: the Claude Code MCP client expects
            // the standard session flow (initialize → `Mcp-Session-Id` →
            // subsequent requests), and a stateless server stalls its multi-step
            // handshake.
            let mut config = StreamableHttpServerConfig::default();
            config.stateful_mode = true;
            config.cancellation_token = cancel.clone();
            let factory = move || {
                Ok::<_, std::io::Error>(ReportServer {
                    check_id: id,
                    reported: reported.clone(),
                    reports: reports_for_check.clone(),
                    notify: notify.clone(),
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
            reports,
            notifiers,
        })
    }

    /// The full endpoint URL an agent should connect to for `check_id`.
    pub fn endpoint_url(&self, check_id: CheckId) -> String {
        format!("{}{}", self.base_url, endpoint_path(check_id))
    }

    /// The notifier + result store for `check_id`, so a caller can await the
    /// check's report and read it once it arrives.
    pub fn report_handle(&self, check_id: CheckId) -> (Arc<Notify>, ReportStore) {
        (self.notifiers[&check_id].clone(), self.reports.clone())
    }

    /// The verdict recorded for `check_id`, if any.
    pub fn report_for(&self, check_id: CheckId) -> Option<CheckReport> {
        self.reports.lock().unwrap().get(&check_id).cloned()
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

    fn report_server(check_id: CheckId) -> (ReportServer, ReportStore, Arc<Notify>) {
        let reports: ReportStore = Arc::new(Mutex::new(HashMap::new()));
        let notify = Arc::new(Notify::new());
        let server = ReportServer {
            check_id,
            reported: Arc::new(AtomicBool::new(false)),
            reports: reports.clone(),
            notify: notify.clone(),
        };
        (server, reports, notify)
    }

    #[test]
    fn record_enforces_single_call_and_delivers() {
        let (server, reports, _notify) = report_server(7);

        assert!(server.record(CheckReport {
            success: true,
            evidence: Some("ok".into())
        }));
        // Duplicate is ignored and does not overwrite.
        assert!(!server.record(CheckReport {
            success: false,
            evidence: None
        }));

        let stored = reports.lock().unwrap().get(&7).cloned().expect("recorded");
        assert!(stored.success);
        assert_eq!(stored.evidence.as_deref(), Some("ok"));
        assert_eq!(reports.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn record_wakes_a_waiter() {
        let (server, _reports, notify) = report_server(0);
        // A report that lands before the wait still wakes it (notify_one permit).
        server.record(CheckReport {
            success: true,
            evidence: None,
        });
        // Should return promptly rather than hang.
        tokio::time::timeout(std::time::Duration::from_secs(1), notify.notified())
            .await
            .expect("notified");
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
        let server = ResultServer::start(&[0, 1, 2]).await.unwrap();
        assert!(server.endpoint_url(1).ends_with("/checks/1"));
        assert!(server.endpoint_url(1).starts_with("http://127.0.0.1:"));
        // No reports arrived.
        assert!(server.report_for(1).is_none());
        let _ = server.report_handle(2);
        server.shutdown().await;
    }
}
