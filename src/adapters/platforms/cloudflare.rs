use std::path::PathBuf;

use crate::{
    Shutdownable,
    adapters::{backend::PlatformConfig, cloudflare::CloudflareClient as Client},
    artifacts::CloudflareFileManifest,
    fs::wrangler::Wrangler,
    subsystems::ShutdownResult,
};

use super::Platform;
use async_trait::async_trait;
use derive_getters::Getters;
use miette::Result;
use tracing::info;

#[derive(Getters)]
pub struct CloudflareWorkerPlatform {
    client: Client,
    project_dir: PathBuf,
    wrangler: Wrangler,
}

impl CloudflareWorkerPlatform {
    pub fn new(client: Client, project_dir: PathBuf, wrangler: Wrangler) -> Self {
        Self {
            client,
            project_dir,
            wrangler,
        }
    }
}

#[async_trait]
impl Platform for CloudflareWorkerPlatform {
    fn get_config(&self) -> PlatformConfig {
        PlatformConfig::CloudflareWorker {
            account_id: self.client.account_id().clone(),
            worker_name: self.client.worker_name().clone(),
        }
    }

    async fn deploy(&mut self) -> Result<(String, String)> {
        info!("Deploying Worker!");
        let baseline_version_id = self.client.get_current_version().await?;

        // First, we process create a manifest of the files to upload
        let file_manifest = CloudflareFileManifest::new(&self.project_dir).await?;

        // Next, upload the files and any potentially new metadata from the Wrangler file
        let upload_version_response = self
            .client
            .upload_version(&file_manifest, self.wrangler.clone())
            .await?;

        // Finally, after the files have been uploaded, we need to update the routes, if there are any listed in the wrangler file
        // NOTE: we do this after the upload since there are more things that could go wrong with the upload
        // and we don't want to update the routes if the upload fails.
        if self.wrangler.routes().is_some() {
            self.client
                .update_routes(self.wrangler.routes().as_ref().unwrap().clone())
                .await?;
        }

        Ok((baseline_version_id, upload_version_response.id))
    }

    async fn yank_canary(&mut self) -> Result<()> {
        // In Cloudflare, Workers are both the platform and ingress
        // so we don't need to yank the canary here, we just set the deployment
        // percetage to 0.
        Ok(())
    }

    async fn delete_canary(&mut self) -> Result<()> {
        // Cloudflare Workers should not be deleted.
        Ok(())
    }

    async fn promote_rollout(&mut self) -> Result<()> {
        // In Cloudflare, Workers are both the platform and ingress
        // so we don't need to promote the canary here, we just set the deployment
        // percetage to 100.
        Ok(())
    }
}

#[async_trait]
impl Shutdownable for CloudflareWorkerPlatform {
    async fn shutdown(&mut self) -> ShutdownResult {
        // When we get the shutdown signal, we don't want to do anything in the platform
        Ok(())
    }
}
