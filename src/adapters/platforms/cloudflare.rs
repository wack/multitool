use std::path::PathBuf;

use crate::{
    Shutdownable, adapters::cloudflare::CloudFlareClient as Client, artifacts::CloudFlareManifest,
    fs::FileSystem, subsystems::ShutdownResult,
};

use super::Platform;
use async_trait::async_trait;
use miette::{IntoDiagnostic, Result};

pub struct Deployment {
    client: Client,
    fs: FileSystem,
}

impl Deployment {
    pub fn new(client: Client, fs: FileSystem) -> Self {
        Self { client, fs }
    }

    pub async fn initiate_upload_session(&self) -> Result<()> {
        // To initiate a new upload session, we must first create
        // a manifest of all of the file we intend to upload.
        let project_root = self.select_project_root()?;
        // Create a new CloudFlare Manifest (not a Multi manifest) using
        // the project root as the source root for the CF Worker.
        let manifest = CloudFlareManifest::new(&project_root).await?;
        // Initiate the upload session.
        todo!()
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
impl Platform for Deployment {
    /// This function deploys the project directory as a CloudFlare Worker.
    async fn deploy(&mut self) -> Result<String> {
        // Asset upload for workers is a slightly tricky process. It
        // happens in three phases:
        // 1. First, you start an upload session.
        // 2. Then, you upload your assets.
        // 3. Finally, you sign off the session and receive a deployment token,
        //    which you can then use to release your Worker.
        todo!()
    }

    async fn yank_canary(&mut self) -> Result<()> {
        todo!()
    }

    async fn delete_canary(&mut self) -> Result<()> {
        todo!()
    }

    async fn promote_rollout(&mut self) -> Result<()> {
        todo!()
    }
}

#[async_trait]
impl Shutdownable for Deployment {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!();
    }
}
