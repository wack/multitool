use crate::{adapters::cloudflare::CloudFlareClient as Client, subsystems::ShutdownResult, Shutdownable};

use super::Platform;
use async_trait::async_trait;
use miette::Result;

pub struct Deployment {
    // TODO
    client: Client,
}

impl Deployment {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Platform for Deployment {
    async fn deploy(&mut self) -> Result<String> {
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
