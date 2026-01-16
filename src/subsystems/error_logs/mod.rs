#![allow(dead_code)]
// This module temporarily allows dead code, because we initially
// built it to demo the Agentic SRE functionality, which we are
// not currently productionizing. We will remove this code if we
// choose not to productionize, or make use of it if we do.

use std::time::Duration;

use crate::adapters::{BackendClient, RolloutMetadata};
use bon::bon;
use chrono::Utc;
use kameo::actor::{ActorRef, Spawn, WeakActorRef};
use kameo::error::{ActorStopReason, Infallible};
use kameo::mailbox;
use kameo::Actor;
use miette::Result;
use tokio::time::interval;
use tracing::{debug, trace};

use crate::adapters::{CloudflareClient, backend::MonitorConfig};

/// The frequency with which we poll Cloudflare for error logs.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);

// The name of the error logs subsystem.
// pub const ERROR_LOGS_SUBSYSTEM_NAME: &str = "errorlogs";

/// Arguments for ErrorLogsController initialization.
pub struct ErrorLogsControllerArgs {
    pub metadata: RolloutMetadata,
    pub backend: BackendClient,
    pub cloudflare_client: CloudflareClient,
    pub worker_name: String,
}

/// The ErrorLogsController is responsible for periodically fetching
/// error logs from Cloudflare.
pub struct ErrorLogsController {
    backend: BackendClient,
    cloudflare_client: CloudflareClient,
    worker_name: String,
    metadata: RolloutMetadata,
}

impl Actor for ErrorLogsController {
    type Args = ErrorLogsControllerArgs;
    type Error = Infallible;

    fn name() -> &'static str {
        "ErrorLogsController"
    }

    async fn on_start(
        args: Self::Args,
        _actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        debug!("ErrorLogsController started");

        // Spawn the background polling task
        let backend = args.backend.clone();
        let cloudflare_client = args.cloudflare_client.clone();
        let worker_name = args.worker_name.clone();
        let metadata = args.metadata.clone();

        tokio::spawn(async move {
            let mut timer = interval(DEFAULT_POLL_INTERVAL);

            loop {
                timer.tick().await;
                trace!("Polling for error logs...");
                let to_time = Utc::now();
                let from_time = to_time - chrono::Duration::seconds(60);

                match cloudflare_client
                    .collect_errors(worker_name.clone(), from_time, to_time)
                    .await
                {
                    Ok(error_logs) => {
                        for log in error_logs {
                            let full_path = format!("{} {}", log.method, log.path);
                            if let Err(e) = backend.upload_errors(&metadata, full_path, log.status_code, log.logs).await {
                                tracing::error!("Failed to upload error logs: {}", e);
                            }
                        }
                    }
                    Err(err) => {
                        tracing::error!("Failed to collect error logs: {}", err);
                    }
                }
                trace!("Errors polled successfully");
            }
        });

        Ok(Self {
            backend: args.backend,
            cloudflare_client: args.cloudflare_client,
            worker_name: args.worker_name,
            metadata: args.metadata,
        })
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        reason: ActorStopReason,
    ) -> std::result::Result<(), Self::Error> {
        debug!("ErrorLogsController stopped: {:?}", reason);
        Ok(())
    }
}

#[bon]
impl ErrorLogsController {
    #[builder]
    pub fn new(
        metadata: RolloutMetadata,
        backend: BackendClient,
        monitor: MonitorConfig,
    ) -> Result<ErrorLogsControllerArgs> {
        match monitor {
            MonitorConfig::CloudflareWorkersObservability {
                account_id,
                worker_name,
                api_token,
            } => {
                let cloudflare_client =
                    CloudflareClient::new(account_id, worker_name.clone(), &api_token);

                Ok(ErrorLogsControllerArgs {
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

    /// Spawn the controller and return the actor reference.
    pub fn spawn_controller(args: ErrorLogsControllerArgs) -> ActorRef<Self> {
        Self::spawn_with_mailbox(args, mailbox::unbounded())
    }
}
