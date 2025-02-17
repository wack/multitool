use async_trait::async_trait;
use miette::Result;

use crate::stats::Observation;

pub use cloudwatch::CloudWatch;

#[async_trait]
pub trait Monitor {
    type Item: Observation;
    async fn query(&mut self) -> Result<Vec<Self::Item>>;
}

mod cloudwatch;
