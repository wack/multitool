use async_trait::async_trait;
use bon::bon;
use miette::Result;

use super::Platform;

// TODO: I probably have to pass in the Artifact here.
pub struct LambdaPlatform {
    region: String,
    name: String,
}

#[bon]
impl LambdaPlatform {
    #[builder]
    pub fn new(region: String, name: String) -> Self {
        Self { region, name }
    }
}

#[async_trait]
impl Platform for LambdaPlatform {
    async fn deploy(&mut self) -> Result<()> {
        todo!()
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        todo!()
    }

    async fn promote_canary(&mut self) -> Result<()> {
        todo!()
    }
}
