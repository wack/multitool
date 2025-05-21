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
impl Shutdownable for CloudflareWorkerPlatform {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!();
    }
}
