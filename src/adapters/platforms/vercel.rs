#[cfg(feature = "vercel")]
use std::path::PathBuf;

#[cfg(feature = "vercel")]
use crate::{
    Shutdownable,
    adapters::{backend::PlatformConfig, vercel::VercelClient as Client},
    artifacts::VercelFileManifest,
    subsystems::ShutdownResult,
};

#[cfg(feature = "vercel")]
use super::Platform;
#[cfg(feature = "vercel")]
use async_trait::async_trait;
#[cfg(feature = "vercel")]
use derive_getters::Getters;
#[cfg(feature = "vercel")]
use miette::Result;
#[cfg(feature = "vercel")]
use tracing::info;

#[cfg(feature = "vercel")]
#[derive(Getters)]
pub struct VercelPlatform {
    client: Client,
    project_dir: PathBuf,
    project_name: String,
}

#[cfg(feature = "vercel")]
impl VercelPlatform {
    pub fn new(client: Client, project_dir: PathBuf, project_name: String) -> Self {
        Self {
            client,
            project_dir,
            project_name,
        }
    }
}

#[cfg(feature = "vercel")]
#[async_trait]
impl Platform for VercelPlatform {
    fn get_config(&self) -> PlatformConfig {
        PlatformConfig::Vercel {
            project_name: self.project_name.clone(),
            team_id: self.client.team_id().clone(),
        }
    }

    async fn deploy(&mut self) -> Result<(String, String)> {
        info!("Deploying to Vercel!");

        // Get the current deployment ID as baseline
        let baseline_version_id = self.client.get_current_deployment().await?;

        // 1. Create a manifest of the files to upload with SHA1 hashes
        let file_manifest = VercelFileManifest::new(&self.project_dir).await?;

        // 2. Upload files in parallel using tokio WaitGroup pattern
        self.client.upload_files(&file_manifest).await?;

        // 3. Create the deployment
        let deployment_response = self
            .client
            .create_deployment(&file_manifest, &self.project_name)
            .await?;

        info!("Vercel deployment created: {}", deployment_response.url);

        // Return baseline and new canary deployment IDs
        Ok((baseline_version_id, deployment_response.id))
    }

    async fn yank_canary(&mut self) -> Result<()> {
        // For Vercel, we handle traffic through the ingress layer
        // The platform doesn't need to yank the canary
        Ok(())
    }

    async fn delete_canary(&mut self) -> Result<()> {
        // Vercel deployments are immutable and can remain
        // We don't need to delete them
        Ok(())
    }

    async fn promote_rollout(&mut self) -> Result<()> {
        // For Vercel, promotion is handled through the ingress layer
        // by updating which deployment receives production traffic
        Ok(())
    }
}

#[cfg(feature = "vercel")]
#[async_trait]
impl Shutdownable for VercelPlatform {
    async fn shutdown(&mut self) -> ShutdownResult {
        // When we get the shutdown signal, we don't need to do anything
        // Vercel deployments persist
        Ok(())
    }
}
