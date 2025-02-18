use async_trait::async_trait;
use miette::Result;

use crate::{stats::Observation, Shutdownable};

pub use cloudwatch::CloudWatch;

pub type BoxedMonitor<T> = Box<dyn Monitor<Item = T> + Send>;

#[async_trait]
pub trait Monitor: Shutdownable {
    type Item: Observation;
    async fn query(&mut self) -> Result<Vec<Self::Item>>;
}

mod cloudwatch;
