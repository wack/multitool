use std::time::Duration;

use async_trait::async_trait;
use bon::bon;
use chrono::Utc;
use miette::{Report, Result, miette};
use tokio::{select, time::interval};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use tracing::info;

use crate::adapters::{CloudflareClient as Client, backend::MonitorConfig};

/// The frequency with which we poll Cloudflare for error logs.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// The name of the error logs subsystem.
pub const ERROR_LOGS_SUBSYSTEM_NAME: &str = "errorlogs";

/// The ErrorLogsController is responsible for periodically fetching
/// error logs from Cloudflare.
pub struct ErrorLogsController {
    client: Client,
    worker_name: String,
}

#[bon]
impl ErrorLogsController {
    #[builder]
    pub fn new(monitor: MonitorConfig) -> Result<Self> {
        match monitor {
            MonitorConfig::CloudflareWorkersObservability {
                account_id,
                worker_name,
                api_token,
            } => {
                let client = Client::new(account_id, worker_name.clone(), &api_token);

                Ok(Self {
                    client,
                    worker_name,
                })
            }
            _ => Err(miette::miette!(
                "ErrorLogsController only supports Cloudflare monitors"
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
                    let to_time = Utc::now();
                    let from_time = to_time - chrono::Duration::seconds(60);

                    match self.client
                        .collect_errors(self.worker_name.clone(), from_time, to_time)
                        .await
                    {
                        Ok(error_log_groups) => {
                            if !error_log_groups.is_empty() {
                                // Each group of logs corresponds to a single invocation
                                // of the worker, so we want to send each chunk of logs
                                // to Aviary separately.
                                for error_logs in error_log_groups {
                                    // TODO:send these logs to Aviary
                                    info!(
                                        "Found {} error(s) in the last minute for worker '{}'",
                                        error_logs.len(),
                                        self.worker_name
                                    );

                                }

                            }
                        }
                        Err(err) => {
                            tracing::error!("Failed to collect error logs: {}", err);
                        }
                    }
                }
            }
        }
    }
}
