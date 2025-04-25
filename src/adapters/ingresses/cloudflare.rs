use crate::{
    Shutdownable, WholePercent, adapters::CloudFlareClient as Client, subsystems::ShutdownResult,
};

use super::Ingress;
use async_trait::async_trait;
use miette::Result;

pub struct GradualDeployment {
    client: Client,
}

impl GradualDeployment {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Ingress for GradualDeployment {
    async fn release_canary(&mut self, platform_id: String) -> Result<()> {
        todo!()
    }

    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        todo!();
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        todo!()
    }

    async fn promote_canary(&mut self) -> Result<()> {
        todo!()
    }
}

#[async_trait]
impl Shutdownable for GradualDeployment {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!();
    }
}
