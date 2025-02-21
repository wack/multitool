use async_trait::async_trait;
use miette::Result;

pub use client::MultiToolBackend;
pub use config::BackendConfig;
use derive_getters::Getters;

use crate::{
    artifacts::LambdaZip, fs::Session, metrics::ResponseStatusCode, stats::CategoricalObservation,
};

use super::{BoxedIngress, BoxedMonitor, BoxedPlatform};

mod client;
mod config;

/// Backend references the MultiTool backend.
#[async_trait]
pub trait BackendClient {
    /// Given the workspace name and the application name, fetch
    /// the configuration of the application.
    async fn fetch_config(
        &self,
        workspace: &str,
        application: &str,
        artifact: LambdaZip,
    ) -> Result<ApplicationConfig>;

    /// This fuction logs the user into the backend by exchanging these credentials
    /// with the backend server.
    async fn exchange_creds(&self, email: &str, password: &str) -> Result<Session>;
}

#[derive(Getters)]
pub struct ApplicationConfig {
    pub platform: BoxedPlatform,
    pub ingress: BoxedIngress,
    pub monitor: BoxedMonitor<CategoricalObservation<5, ResponseStatusCode>>,
}
