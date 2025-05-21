use crate::{
    Shutdownable, adapters::cloudflare::CloudflareClient as Client, subsystems::ShutdownResult,
};

use super::Platform;
use async_trait::async_trait;
use miette::Result;

pub struct CloudflareWorkerPlatform {
    client: Client,
    worker_name: String,
    account_id: String,
}

impl CloudflareWorkerPlatform {
    pub fn new(client: Client, account_id: String, worker_name: String) -> Self {
        Self {
            client,
            account_id,
            worker_name,
        }
    }
}

#[async_trait]
impl Platform for CloudflareWorkerPlatform {
    async fn deploy(&mut self) -> Result<(String, String)> {
        todo!()
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
