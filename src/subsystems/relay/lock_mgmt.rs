use crate::adapters::{BackendClient, DeploymentMetadata, StateId};

pub(super) struct LockManagementSubsystem {
    /// We use this client to refresh locks.
    backend: BackendClient,
    /// This field describes the current active deployment.
    /// This is context we pass to the backend on each request.
    meta: DeploymentMetadata,
    /// This is the state that this manager is locking.
    state_id: StateId,
}
