use bon::{Builder, bon};
use derive_getters::Getters;
use miette::{IntoDiagnostic, Result};
use multitool_sdk::models::DeploymentState;
use std::sync::Arc;
use tokio::{
    sync::{mpsc, oneshot},
    time::Duration,
};

pub(crate) type WorkspaceId = u32;
pub(crate) type ApplicationId = u32;
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
    state: DeploymentState,
    /// How often the lease must be renewed.
    frequency: Duration,
    /// When the state has been effected, release the lock we have
    /// on the state. This channel signals to the thread managing
    /// the lock that it can tell the backend to release
    /// the lock because the state has been effected.
    task_done: Arc<mpsc::Sender<oneshot::Sender<()>>>,
}

#[bon]
impl LockedState {
    #[builder]
    pub(crate) fn new(
        state: DeploymentState,
        frequency: Duration,
        task_done: mpsc::Sender<oneshot::Sender<()>>,
    ) -> Self {
        Self {
            state,
            frequency,
            task_done: Arc::new(task_done),
        }
    }

    pub(crate) async fn mark_done(&mut self) -> Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.task_done.send(sender).await.into_diagnostic()?;
        receiver.await.into_diagnostic()
    }
}
