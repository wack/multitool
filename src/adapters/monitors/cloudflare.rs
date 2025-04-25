use async_trait::async_trait;

use crate::{
    Shutdownable, adapters::CloudFlareClient as Client, metrics::ResponseStatusCode,
    stats::CategoricalObservation, subsystems::ShutdownResult,
};
use miette::Result;

use super::Monitor;

pub struct CloudFlareMonitor {
    client: Client,
}

impl CloudFlareMonitor {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Monitor for CloudFlareMonitor {
    type Item = CategoricalObservation<5, ResponseStatusCode>;

    async fn query(&mut self) -> Result<Vec<Self::Item>> {
        todo!()
    }
}

#[async_trait]
impl Shutdownable for CloudFlareMonitor {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!();
    }
}
