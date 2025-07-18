use std::path::PathBuf;

use crate::{
    Shutdownable,
    adapters::{
        backend::PlatformConfig,
        cloudflare::{CloudflareClient as Client, uploads::UploadVersionRequest},
    },
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
    artifact_path: PathBuf,
    wrangler: Wrangler,
}

impl CloudflareWorkerPlatform {
    pub fn new(client: Client, artifact_path: PathBuf, wrangler: Wrangler) -> Self {
        Self {
            client,
            artifact_path,
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

        // 1. First, we create a manifest of the files to upload
        let file_manifest = CloudflareFileManifest::new(&self.artifact_path).await?;

        // Convert our wrangler file to the Request format Cloudflare expects
        let request = UploadVersionRequest::from(self.wrangler.clone());

        // 2. Upload the files and any potentially new metadata from the Wrangler file
        let upload_version_request = self.client.upload_version(&file_manifest, &request).await?;

        // 3. After the files have been uploaded, we need to update the routes, if there are any listed in the wrangler file
        // NOTE: we do this after the upload since there are more things that could go wrong with the upload
        // and we don't want to update the routes if the upload fails.
        if self.wrangler.routes().is_some() {
            self.client
                .sync_routes(self.wrangler.routes().as_ref().unwrap().clone())
                .await?;
        }

        Ok((baseline_version_id, upload_version_request.id))
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
