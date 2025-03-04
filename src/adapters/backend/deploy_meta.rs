use bon::Builder;
use derive_getters::Getters;
use multitool_sdk::models::DeploymentState;
use tokio::{sync::mpsc, time::Duration};
use uuid::Uuid;

pub(crate) type WorkspaceId = Uuid;
pub(crate) type ApplicationId = Uuid;
pub(crate) type DeploymentId = u64;

/// DeploymentMetadata captures the relevant parameters for a particular
/// deployment. This struct is mostly used in conjuction with a `BackendClient`
/// to hold the context for the current deployment.
#[derive(Builder, Getters, Clone)]
pub(crate) struct DeploymentMetadata {
    workspace_id: WorkspaceId,
    application_id: ApplicationId,
    deployment_id: DeploymentId,
}

#[derive(Getters, Clone)]
pub(crate) struct LockedState {
    id: DeploymentState,
    /// How often the lease must be renewed.
    period: Duration,
    /// When the state has been effected, release the lock we have
    /// on the state. This channel signals to the thread managing
    /// the lock that it can tell the backend to release
    /// the lock because the state has been effected.
    task_done: mpsc::Sender<()>,
}
