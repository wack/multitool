use crate::{
    Shutdownable, WholePercent,
    adapters::{
        CloudflareClient as Client,
        backend::IngressConfig,
        cloudflare::deployments::{CreateDeploymentRequest, DeploymentStrategy, DeploymentVersion},
    },
    subsystems::ShutdownResult,
};

use super::Ingress;
use async_trait::async_trait;
use derive_getters::Getters;
use miette::Result;
use tracing::{debug, info};

#[derive(Getters)]
pub struct CloudflareWorkerIngress {
    client: Client,
    // The name of the worker being monitored
    worker_name: String,
    // The version id of the baseline version
    control_version_id: Option<String>,
    // The version id of the canary version
    canary_version_id: Option<String>,
}

impl CloudflareWorkerIngress {
    pub fn new(client: Client, worker_name: String) -> Self {
        Self {
            client,
            worker_name,
            control_version_id: None,
            canary_version_id: None,
        }
    }
}

#[async_trait]
impl Ingress for CloudflareWorkerIngress {
    fn get_config(&self) -> IngressConfig {
        IngressConfig::CloudflareWorker {
            account_id: self.client.account_id().clone(),
            worker_name: self.worker_name().clone(),
        }
    }

    async fn release_canary(
        &mut self,
        baseline_version_id: String,
        canary_version_id: String,
    ) -> Result<()> {
        debug!("Releasing canary in Cloudflare!");

        // First, save these values to this struct
        self.control_version_id = Some(baseline_version_id.clone());
        self.canary_version_id = Some(canary_version_id.clone());

        // Finally, we can create the config and make the request
        let control_version = DeploymentVersion::builder()
            .percentage(100)
            .version_id(baseline_version_id.clone())
            .build();
        let canary_version = DeploymentVersion::builder()
            .percentage(0)
            .version_id(canary_version_id.clone())
            .build();
        let deployment_request = CreateDeploymentRequest::builder()
            .strategy(DeploymentStrategy::Percentage)
            .versions(vec![control_version, canary_version])
            .build();

        self.client
            .create_deployment(self.worker_name(), deployment_request)
            .await
    }

    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        info!("Setting Cloudflare canary traffic to {percent}.");
        let control_version = DeploymentVersion::builder()
            .percentage((100 - percent.clone().as_i32()) as u64)
            .version_id(self.control_version_id.clone().unwrap())
            .build();
        let canary_version = DeploymentVersion::builder()
            .percentage(percent.as_i32() as u64)
            .version_id(self.canary_version_id.clone().unwrap())
            .build();
        let deployment_request = CreateDeploymentRequest::builder()
            .strategy(DeploymentStrategy::Percentage)
            .versions(vec![control_version, canary_version])
            .build();

        self.client
            .create_deployment(self.worker_name(), deployment_request)
            .await
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        info!("Rolling back canary in Cloudflare.");
        let control_version = DeploymentVersion::builder()
            .percentage(100)
            .version_id(self.control_version_id.clone().unwrap())
            .build();
        let deployment_request = CreateDeploymentRequest::builder()
            .strategy(DeploymentStrategy::Percentage)
            .versions(vec![control_version])
            .build();

        // Clear canary version ID after rolling back since there's now
        // no canary version and so we don't try to roll it back (again) during shutdown.
        self.canary_version_id = None;

        self.client
            .create_deployment(self.worker_name(), deployment_request)
            .await
    }

    async fn promote_canary(&mut self) -> Result<()> {
        info!("Promoting canary in Cloudflare!");
        let canary_version = DeploymentVersion::builder()
            .percentage(100)
            .version_id(self.canary_version_id.clone().unwrap())
            .build();
        let deployment_request = CreateDeploymentRequest::builder()
            .strategy(DeploymentStrategy::Percentage)
            .versions(vec![canary_version])
            .build();

        // Clear canary version ID after promotion since it's now
        // the control version and so we don't try to roll it back during shutdown.
        self.canary_version_id = None;

        self.client
            .create_deployment(self.worker_name(), deployment_request)
            .await
    }
}

#[async_trait]
impl Shutdownable for CloudflareWorkerIngress {
    async fn shutdown(&mut self) -> ShutdownResult {
        // If there's no canary version ID set, there are 2 possibilities:
        // 1. The canary was never released, so there's nothing to rollback.
        // 2. The canary was already promoted, so there's nothing to rollback.
        if self.canary_version_id.is_none() {
            debug!("No canary version ID set, nothing to rollback.");
            return Ok(());
        }
        self.rollback_canary().await?;
        Ok(())
    }
}
