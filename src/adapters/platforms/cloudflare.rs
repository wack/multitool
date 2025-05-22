use std::path::PathBuf;

use crate::{
    Shutdownable, adapters::cloudflare::CloudflareClient as Client, artifacts::CloudflareManifest,
    fs::FileSystem, subsystems::ShutdownResult,
};

use super::Platform;
use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};

pub struct CloudflareWorkerPlatform {
    client: Client,
    fs: FileSystem,
}

impl CloudflareWorkerPlatform {
    pub fn new(client: Client, fs: FileSystem) -> Self {
        Self { client, fs }
    }

    /// Find or determine the project root, either by using the project's
    /// manifest file, or the current working directory if none was found.
    fn select_project_root(&self) -> Result<PathBuf> {
        match self.fs.project_dir() {
            Err(err) => Err(err),
            Ok(Some(path)) => Ok(path),
            Ok(None) => std::env::current_dir().into_diagnostic(),
        }
    }
}

#[async_trait]
impl Platform for CloudflareWorkerPlatform {
    async fn deploy(&mut self) -> Result<(String, String)> {
        let baseline_version_id = self.client.get_current_version().await?;

        // Asset upload for workers is a slightly tricky process. It
        // happens in three phases:
        // 1. First, we create a manifest and send it to start an upload session.
        let project_root = self.select_project_root()?;
        let manifest = CloudflareManifest::new(&project_root).await?;
        let upload_session_response = self.client.create_assets_upload_session(&manifest).await?;

        let mut completion_jwt = upload_session_response.jwt.clone();

        let mut keep_assets = false;
        // 2. Then, you upload your assets, but only if Cloudflare wants them
        // by telling us which files to upload in buckets.
        if !upload_session_response.buckets.is_empty() {
            for bucket in upload_session_response.buckets {
                let upload_response = self
                    .client
                    .upload_assets(&upload_session_response.jwt, bucket, manifest.clone())
                    .await?;
                completion_jwt = upload_response.jwt;
            }
        } else {
            keep_assets = true;
        }

        // 3. Finally, you sign off the session and receive a deployment token,
        //    which you can then use to release your Worker.
        let upload_version_request = self
            .client
            .upload_version(completion_jwt, keep_assets)
            .await?;

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
        todo!();
    }
}
