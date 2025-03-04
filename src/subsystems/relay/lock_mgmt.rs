use async_trait::async_trait;
use bon::bon;
use multitool_sdk::models::DeploymentState;

use crate::{
    Shutdownable,
    adapters::{BackendClient, DeploymentMetadata},
    subsystems::ShutdownResult,
};

pub(super) struct LockManager {
    /// We use this client to refresh locks.
    backend: BackendClient,
    /// This field describes the current active deployment.
    /// This is context we pass to the backend on each request.
    meta: DeploymentMetadata,
    /// This is the state that this manager is locking.
    state: DeploymentState,
}

#[bon]
impl LockManager {
    #[builder]
    pub(super) fn new(
        backend: BackendClient,
        metadata: DeploymentMetadata,
        state: DeploymentState,
    ) -> Self {
        Self {
            backend,
            state,
            meta: metadata,
        }
    }
}

#[async_trait]
impl Shutdownable for LockManager {
    async fn shutdown(&mut self) -> ShutdownResult {
        // Release any of the locks we've taken.
        todo!()
    }
}
