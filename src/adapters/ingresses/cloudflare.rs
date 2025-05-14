use crate::{
    Shutdownable, WholePercent,
    adapters::{
        CloudflareClient as Client,
        cloudflare::deployments::{
            CreateDeploymentRequest, DeploymentStrategy, DeploymentVersionConfig,
        },
    },
    subsystems::ShutdownResult,
};

use super::Ingress;
use async_trait::async_trait;
use miette::Result;
use tracing::{debug, info};

pub struct GradualDeployment {
    client: Client,
    // Cloudflare account id
    account_id: String,
    // Cloudflare worker name
    worker_name: String,
    // The version id of the baseline version
    control_version_id: Option<String>,
    // The version id of the canary version
    canary_version_id: Option<String>,
}

impl GradualDeployment {
    pub fn new(client: Client, account_id: String, worker_name: String) -> Self {
        Self {
            client,
            account_id,
            worker_name,
            control_version_id: None,
            canary_version_id: None,
        }
    }
}

#[async_trait]
impl Ingress for GradualDeployment {
    async fn release_canary(&mut self, canary_version_id: String) -> Result<()> {
        debug!("Releasing canary in Cloudflare!");

        // First, we need to get the current running version
        let control_version_id = self
            .client
            .get_current_version(self.account_id.clone(), self.worker_name.clone())
            .await?;

        // Next, we need to save these values to this struct
        self.control_version_id = Some(control_version_id.clone());
        self.canary_version_id = Some(canary_version_id.clone());

        // Finally, we can create the config and make the request
        let control_version = DeploymentVersionConfig::builder()
            .percentage(100)
            .version_id(control_version_id.clone())
            .build();
        let canary_version = DeploymentVersionConfig::builder()
            .percentage(0)
            .version_id(canary_version_id.clone())
            .build();
        let deployment_request = CreateDeploymentRequest::builder()
            .strategy(DeploymentStrategy::Percentage)
            .versions(vec![control_version, canary_version])
            .build();

        self.client
            .create_deployment(
                self.account_id.clone(),
                self.worker_name.clone(),
                deployment_request,
            )
            .await
    }

    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        info!("Setting Cloudflare canary traffic to {percent}.");
        let control_version = DeploymentVersionConfig::builder()
            .percentage((100 - percent.clone().as_i32()) as u64)
            .version_id(
                self.control_version_id
                    .clone()
                    .unwrap_or("No control version id found".to_string()),
            )
            .build();
        let canary_version = DeploymentVersionConfig::builder()
            .percentage(percent.as_i32() as u64)
            .version_id(
                self.canary_version_id
                    .clone()
                    .unwrap_or("No canary version id found".to_string()),
            )
            .build();
        let deployment_request = CreateDeploymentRequest::builder()
            .strategy(DeploymentStrategy::Percentage)
            .versions(vec![control_version, canary_version])
            .build();

        self.client
            .create_deployment(
                self.account_id.clone(),
                self.worker_name.clone(),
                deployment_request,
            )
            .await
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        info!("Rolling back canary in Cloudflare.");
        let control_version = DeploymentVersionConfig::builder()
            .percentage(100)
            .version_id(
                self.control_version_id
                    .clone()
                    .unwrap_or("No control version id found".to_string()),
            )
            .build();
        let deployment_request = CreateDeploymentRequest::builder()
            .strategy(DeploymentStrategy::Percentage)
            .versions(vec![control_version])
            .build();

        self.client
            .create_deployment(
                self.account_id.clone(),
                self.worker_name.clone(),
                deployment_request,
            )
            .await
    }

    async fn promote_canary(&mut self) -> Result<()> {
        info!("Promoting canary in Cloudflare!");
        let canary_version = DeploymentVersionConfig::builder()
            .percentage(100)
            .version_id(
                self.canary_version_id
                    .clone()
                    .unwrap_or("No canary version id found".to_string()),
            )
            .build();
        let deployment_request = CreateDeploymentRequest::builder()
            .strategy(DeploymentStrategy::Percentage)
            .versions(vec![canary_version])
            .build();

        self.client
            .create_deployment(
                self.account_id.clone(),
                self.worker_name.clone(),
                deployment_request,
            )
            .await
    }
}

#[async_trait]
impl Shutdownable for GradualDeployment {
    async fn shutdown(&mut self) -> ShutdownResult {
        self.rollback_canary().await?;
        Ok(())
    }
}
