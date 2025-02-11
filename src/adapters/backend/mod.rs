use async_trait::async_trait;
use miette::Result;

pub use client::MultiToolBackend;
pub use config::BackendConfig;

use crate::fs::Session;

mod client;
mod config;

/// Backend references the MultiTool backend.
#[async_trait]
pub trait BackendClient {
    /// Given the workspace name and the application name, fetch
    /// the configuration of the application.
    async fn fetch_config(&self, workspace: String, application: String) -> Result<()>;

    /// This fuction logs the user into the backend by exchanging these credentials
    /// with the backend server.
    async fn exchange_creds(&self, email: String, password: String) -> Result<Session>;
}
