use async_trait::async_trait;
use config::{IngressConfig, MonitorConfig, PlatformConfig};
use miette::Result;

pub use client::MultiToolBackend;
pub use config::BackendConfig;
use derive_getters::Getters;

use crate::fs::Session;

mod client;
mod config;

/// Backend references the MultiTool backend.
#[async_trait]
pub trait BackendClient {
    /// Given the workspace name and the application name, fetch
    /// the configuration of the application.
    async fn fetch_config(&self, workspace: &str, application: &str) -> Result<ApplicationConfig>;

    /// This fuction logs the user into the backend by exchanging these credentials
    /// with the backend server.
    async fn exchange_creds(&self, email: &str, password: &str) -> Result<Session>;
}

#[derive(Getters)]
pub struct ApplicationConfig {
    platform: PlatformConfig,
    ingress: IngressConfig,
    monitor: MonitorConfig,
}
