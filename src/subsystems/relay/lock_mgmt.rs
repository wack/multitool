use async_trait::async_trait;

use crate::{
    Shutdownable,
    adapters::{BackendClient, DeploymentMetadata, StateId},
    subsystems::ShutdownResult,
};

pub(super) struct LockManagementSubsystem {
    /// We use this client to refresh locks.
    backend: BackendClient,
    /// This field describes the current active deployment.
    /// This is context we pass to the backend on each request.
    meta: DeploymentMetadata,
    /// This is the state that this manager is locking.
    state_id: StateId,
}

#[async_trait]
impl Shutdownable for LockManagementSubsystem {
    async fn shutdown(&mut self) -> ShutdownResult {
        // Release any of the locks we've taken.
        todo!()
    }
}
