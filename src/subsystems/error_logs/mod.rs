#![allow(dead_code)]
// This module temporarily allows dead code, because we initially
// built it to demo the Agentic SRE functionality, which we are
// not currently productionizing. We will remove this code if we
// choose not to productionize, or make use of it if we do.

use std::time::Duration;

use crate::adapters::{BackendClient, RolloutMetadata};
use async_trait::async_trait;
use bon::bon;
use chrono::Utc;
use miette::{Report, Result};
use tokio::{select, time::interval};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use tracing::trace;

use crate::adapters::{CloudflareClient, backend::MonitorConfig};

/// The frequency with which we poll Cloudflare for error logs.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

// The name of the error logs subsystem.
// pub const ERROR_LOGS_SUBSYSTEM_NAME: &str = "errorlogs";

/// The ErrorLogsController is responsible for periodically fetching
/// error logs from Cloudflare.
pub struct ErrorLogsController {
    backend: BackendClient,
    cloudflare_client: CloudflareClient,
    worker_name: String,
    metadata: RolloutMetadata,
}

#[bon]
impl ErrorLogsController {
    #[builder]
    pub fn new(
        metadata: RolloutMetadata,
        backend: BackendClient,
        monitor: MonitorConfig,
    ) -> Result<Self> {
        match monitor {
            MonitorConfig::CloudflareWorkersObservability {
                account_id,
                worker_name,
                api_token,
            } => {
                let cloudflare_client =
                    CloudflareClient::new(account_id, worker_name.clone(), &api_token);

                Ok(Self {
                    metadata,
                    backend,
                    cloudflare_client,
                    worker_name,
                })
            }
            _ => Err(miette::miette!(
                "Error Logs only supports Cloudflare monitors"
            )),
        }
    }
}

#[async_trait]
impl IntoSubsystem<Report> for ErrorLogsController {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        let mut timer = interval(DEFAULT_POLL_INTERVAL);

        loop {
            select! {
                _ = subsys.on_shutdown_requested() => {
                    return Ok(());
                }
                _ = timer.tick() => {
                    trace!("Polling for error logs...");
                    let to_time = Utc::now();
                    let from_time = to_time - chrono::Duration::seconds(60);

                    match self.cloudflare_client
                        .collect_errors(self.worker_name.clone(), from_time, to_time)
                        .await
                    {
                        Ok(error_logs) => {
                            for log in error_logs {
                                let full_path = format!("{} {}", log.method, log.path);
                                self.backend.upload_errors(&self.metadata, full_path, log.status_code, log.logs).await?;
                            }
                        }
                        Err(err) => {
                            tracing::error!("Failed to collect error logs: {}", err);
                        }
                    }
                    trace!("Errors polled successfully");
                }
            }
        }
    }
}
