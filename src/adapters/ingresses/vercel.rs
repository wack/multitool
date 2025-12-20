#![allow(dead_code)]
// This will not be dead once we plug it into a the Platform.

use crate::{
    Shutdownable, WholePercent, adapters::backend::IngressConfig, subsystems::ShutdownResult,
};

use super::Ingress;
use async_trait::async_trait;
use derive_getters::Getters;
use miette::Result;

// Placeholder: we know we need a Vercel HTTP client here,
// but we don't have one in place just yet.
#[allow(dead_code)]
type VercelClient = ();

#[allow(dead_code)]
#[derive(Getters)]
pub struct Vercel {
    client: VercelClient,
}

impl Vercel {
    pub fn new(client: VercelClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Ingress for Vercel {
    fn get_config(&self) -> IngressConfig {
        todo!();
    }

    async fn release_canary(
        &mut self,
        baseline_version_id: String,
        canary_version_id: String,
    ) -> Result<()> {
        let _ = baseline_version_id;
        let _ = canary_version_id;
        todo!();
    }

    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        let _ = percent;
        todo!();
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        todo!();
    }

    async fn promote_canary(&mut self) -> Result<()> {
        todo!();
    }
}

#[async_trait]
impl Shutdownable for Vercel {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!();
    }
}
