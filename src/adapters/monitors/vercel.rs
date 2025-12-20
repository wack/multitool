#![allow(dead_code)]
// This code won't be dead once we incorporate it into the flow.

use async_trait::async_trait;
use derive_getters::Getters;

use crate::{
    Shutdownable, adapters::backend::MonitorConfig, metrics::ResponseStatusCode,
    stats::CategoricalObservation, subsystems::ShutdownResult,
};
use miette::Result;

use super::Monitor;

type VercelClient = ();

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
impl Monitor for Vercel {
    type Item = CategoricalObservation<5, ResponseStatusCode>;

    fn get_config(&self) -> MonitorConfig {
        todo!();
    }

    async fn query(&mut self) -> Result<Vec<Self::Item>> {
        todo!();
    }

    async fn set_canary_version_id(&mut self, canary_version_id: String) -> Result<()> {
        let _ = canary_version_id;
        todo!();
    }

    // TODO: standardize naming to baseline when working outside of statistics packages
    async fn set_baseline_version_id(&mut self, baseline_version_id: String) -> Result<()> {
        let _ = baseline_version_id;
        todo!();
    }
}

#[async_trait]
impl Shutdownable for Vercel {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!();
    }
}
