use bon::Builder;
use chrono::{DateTime, Utc};
use derive_getters::Getters;
use tokio::time::Duration;
use uuid::Uuid;

pub(crate) type WorkspaceId = Uuid;
pub(crate) type ApplicationId = Uuid;
pub(crate) type DeploymentId = u64;
pub(crate) type StateId = u64;

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
pub(crate) struct StateLock {
    id: StateId,
    expiry: DateTime<Utc>,
    // How often the lease must be renewed.
    period: Duration,
}
