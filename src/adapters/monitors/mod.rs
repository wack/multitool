use async_trait::async_trait;
use miette::Result;

use crate::{
    Shutdownable,
    metrics::ResponseStatusCode,
    stats::{CategoricalObservation, Observation},
};

/// StatusCode is a type alias for the unwieldly named type on the right.
pub type StatusCode = CategoricalObservation<5, ResponseStatusCode>;

// TODO: For now, we require all monitors to monitor just
// the status code. We may have trouble with the Builder in the
// future because we can't really genericize it. But when we add
// more metrics, we'll upgrade Monitors to handle them all, simultaniously,
// and there may not be a generic parameter on the Monitor type anymore.
pub type BoxedMonitor = Box<dyn Monitor<Item = StatusCode> + Send + Sync>;

pub(crate) use builder::MonitorBuilder;

#[async_trait]
pub trait Monitor: Shutdownable {
    type Item: Observation;
    async fn query(&mut self) -> Result<Vec<Self::Item>>;
}

mod builder;
mod cloudwatch;
