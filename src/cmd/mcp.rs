use crate::adapters::BackendClient;
use crate::fs::SessionFile;
use miette::{IntoDiagnostic as _, Result};
use rmcp::{
    Error as McpError, RoleServer, ServerHandler, ServiceExt, const_string, model::*, schemars,
    service::RequestContext, tool, transport::stdio,
};
use serde_json::json;
use tokio::runtime::Runtime;

use crate::{Terminal, config::LoginSubcommand, fs::FileSystem};

pub struct Mcp {
    terminal: Terminal,
}

#[derive(Clone)]
struct McpServer;

impl McpServer {
    fn new() -> Self {
        Self
    }
}

#[tool(tool_box)]
impl McpServer {
    #[tool(description = "Create a new deployment")]
    fn run(&self) -> Result<CallToolResult, McpError> {
            Command::new("multi")
        .args(["run", "--workspace", "", "--application", ""])
        .output()
        .expect("failed to execute process")
        todo!()
    }

    #[tool(description = "Say hello to the client")]
    fn say_hello(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            "hello there!",
        )]))
    }
}

#[tool(tool_box)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                // .enable_prompts()
                // .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation {
                name: "multitool".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            instructions: Some("TODO".to_string()),
        }
    }
}

impl Mcp {
    pub fn new(terminal: Terminal) -> Self {
        Self { terminal }
    }

    pub fn dispatch(self) -> Result<()> {
        let fs = FileSystem::new()?;
        let rt = Runtime::new().into_diagnostic()?;
        let _guard = rt.enter();
        rt.block_on(async {
            // Load the user's credentials from disk, and error
            // if they're not found.
            let session = fs.load_file(SessionFile)?;
            // Create an instance of the MCP Server.
            let server = McpServer::new()
                .serve(stdio())
                .await
                .inspect_err(|e| {
                    tracing::error!("serving error: {:?}", e);
                })
                .into_diagnostic()?;

            server.waiting().await.into_diagnostic()?;
            Ok(())
        })
    }
}
