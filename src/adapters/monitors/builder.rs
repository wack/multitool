use async_trait::async_trait;
use multitool_sdk::models::MonitorConfig;

use super::BoxedMonitor;

#[async_trait]
trait Builder<T> {
    async fn build(self) -> BoxedMonitor<T>;
}

pub(crate) struct MonitorBuilder {
    config: MonitorConfig,
}

impl MonitorBuilder {
    pub(crate) fn new(config: MonitorConfig) -> Self {
        Self { config }
    }

    pub async fn build<T>(self) -> BoxedMonitor<T> {
        Builder::build(self).await
    }
}

#[async_trait]
impl<T> Builder<T> for MonitorBuilder {
    async fn build(self) -> BoxedMonitor<T> {
        todo!()
    }
}
