use std::ops::Deref;
use std::sync::Arc;

use super::{BoxedIngress, BoxedMonitor, BoxedPlatform, StatusCode};
use crate::fs::UserCreds;
use crate::{fs::Session, metrics::ResponseStatusCode};
use chrono::DateTime;
use miette::miette;
use miette::{IntoDiagnostic, Result, bail};
use multitool_sdk::apis::{Api, ApiClient, configuration::Configuration};
use multitool_sdk::models::{
    ApplicationDetails, ApplicationGroup, CreateResponseCodeMetricsRequest,
    CreateResponseCodeMetricsSuccess, DeploymentStateStatus, LoginRequest, LoginSuccess,
    WorkspaceSummary,
};
use multitool_sdk::models::{DeploymentState, UpdateDeploymentStateRequest};
use tokio::sync::mpsc::Sender;
use tokio::task::JoinSet;
use tokio::time::Duration;

pub(crate) use deploy_meta::*;
use tracing::trace;

// WARNING: This code seriously needs to be cleaned up.
// I wrote this in a sloppy fit while trying to yak shave
// about a million other things.

/// The BackendClient sends requests to the MultiTool SaaS
/// backend. It wraps our generated HTTP bindings.
pub struct BackendClient {
    /// We keep a copy of the OpenAPI config, which is used
    /// in each request.
    conf: Configuration,
    client: ApiClient,
    session: Option<Session>,
    // TODO: Add a method for updating the access token.
}

impl Clone for BackendClient {
    fn clone(&self) -> Self {
        let conf = self.conf.clone();
        Self {
            conf: conf.clone(),
            client: ApiClient::new(Arc::new(conf)),
            session: self.session.clone(),
        }
    }
}

impl BackendClient {
    /// Return a new backend client for the MultiTool backend.
    pub fn new(origin: Option<&str>, session: Option<Session>) -> Result<Self> {
        let conf = BackendConfig::new(origin, session.clone());

        let raw_conf: Configuration = conf.clone().into();

        let client = ApiClient::new(Arc::new(raw_conf.clone()));
        Ok(Self {
            conf: raw_conf,
            client,
            session,
        })
    }

    pub fn is_authenicated(&self) -> Result<()> {
        if self.session.clone().is_some_and(Session::is_not_expired) {
            return Ok(());
        } else {
            bail!("Please login before running this command.");
        }
    }

    pub(crate) async fn lock_state(
        &self,
        meta: &DeploymentMetadata,
        state: &DeploymentState,
        done_sender: Sender<()>,
    ) -> Result<LockedState> {
        trace!("Locking state {}...", state.state_type);
        self.client
            .deployment_states_api()
            .update_deployment_state(
                *meta.workspace_id(),
                *meta.application_id(),
                *meta.deployment_id(),
                state.id,
                UpdateDeploymentStateRequest {
                    status: Some(Some(DeploymentStateStatus::InProgress)),
                },
            )
            .await
            .into_diagnostic()?;

        let locked_state = LockedState::builder()
            .state(state.clone())
            // TODO: we should return this from the API
            .frequency(Duration::from_secs(30))
            .task_done(done_sender)
            .build();

        trace!("State locked successfully");
        Ok(locked_state)
    }

    pub(crate) async fn refresh_lock(
        &self,
        meta: &DeploymentMetadata,
        locked_state: &LockedState,
    ) -> Result<()> {
        trace!("Refreshing {} lock...", locked_state.state().state_type);
        self.client
            .deployment_states_api()
            .refresh_deployment_state(
                *meta.workspace_id(),
                *meta.application_id(),
                *meta.deployment_id(),
                locked_state.state().id,
            )
            .await
            .into_diagnostic()?;
        trace!("Lock refreshed successfully");
        Ok(())
    }

    /// Release the lock on this state without completing it.
    pub(crate) async fn abandon_lock(
        &self,
        meta: &DeploymentMetadata,
        locked_state: &LockedState,
    ) -> Result<()> {
        trace!("Abandoning {} lock", locked_state.state().state_type);
        self.client
            .deployment_states_api()
            .update_deployment_state(
                *meta.workspace_id(),
                *meta.application_id(),
                *meta.deployment_id(),
                locked_state.state().id,
                UpdateDeploymentStateRequest {
                    status: Some(Some(DeploymentStateStatus::Pending)),
                },
            )
            .await
            .into_diagnostic()?;

        trace!("Lock abandoned successfully");
        Ok(())
    }

    /// Poll the backend for pending states that have not yet been
    /// locked/claimed and thus are ready to be locked and processed.
    pub(crate) async fn poll_for_state(
        &self,
        meta: &DeploymentMetadata,
    ) -> Result<Vec<DeploymentState>> {
        trace!("Polling for new states...");
        let response = self
            .client
            .deployment_states_api()
            .list_deployment_states(
                *meta.workspace_id(),
                *meta.application_id(),
                *meta.deployment_id(),
                Some(DeploymentStateStatus::Pending),
            )
            .await
            .into_diagnostic()?;

        trace!("States polled successfully");
        Ok(response.states)
    }

    pub(crate) async fn mark_state_completed(
        &self,
        meta: &DeploymentMetadata,
        locked_state: &LockedState,
    ) -> Result<()> {
        trace!(
            "Marking state {} as completed...",
            locked_state.state().state_type
        );
        self.client
            .deployment_states_api()
            .update_deployment_state(
                *meta.workspace_id(),
                *meta.application_id(),
                *meta.deployment_id(),
                locked_state.state().id,
                UpdateDeploymentStateRequest {
                    status: Some(Some(DeploymentStateStatus::Done)),
                },
            )
            .await
            .into_diagnostic()?;

        trace!("State successfully marked as complete");
        Ok(())
    }

    pub async fn new_deployment(
        &self,
        workspace_id: WorkspaceId,
        application_id: ApplicationId,
    ) -> Result<DeploymentId> {
        trace!("Creating a new deployment");
        let response = self
            .client
            .deployments_api()
            .create_deployment(workspace_id, application_id)
            .await
            .into_diagnostic()?;

        trace!("Deployment created successfully");
        Ok(response.deployment.id)
    }

    /// This fuction logs the user into the backend by exchanging these credentials
    /// with the backend server.
    pub async fn exchange_creds(&self, email: &str, password: &str) -> Result<Session> {
        trace!("Exchanging creds with the backend");
        // • Create and send the request, marshalling the result
        //   into user credentials.
        let req = LoginRequest {
            email: email.to_owned(),
            password: password.to_owned(),
        };
        let creds: UserCreds = self
            .client
            .users_api()
            .login(req)
            .await
            .into_diagnostic()?
            .into();

        trace!("Creds exchanged, login success");
        Ok(Session::User(creds))
    }

    /// Upload a batch of observations to the backend.
    pub(crate) async fn upload_observations(
        &self,
        meta: &DeploymentMetadata,
        data: Vec<StatusCode>,
    ) -> Result<()> {
        trace!("Uploading observations to backend");
        let mut req_waiter = JoinSet::new();

        for item in data {
            let group = match item.group() {
                crate::stats::Group::Control => ApplicationGroup::Baseline,
                crate::stats::Group::Experimental => ApplicationGroup::Canary,
            };
            let req_body = CreateResponseCodeMetricsRequest {
                app_group: group,
                status_2xx_count: item.get_count(&ResponseStatusCode::_2XX) as u32,
                status_4xx_count: item.get_count(&ResponseStatusCode::_4XX) as u32,
                status_5xx_count: item.get_count(&ResponseStatusCode::_5XX) as u32,
            };
            let workspace_id = *meta.workspace_id();
            let application_id = *meta.application_id();
            let deployment_id = *meta.deployment_id();
            let cloned_client = self.clone();
            req_waiter.spawn(async move {
                cloned_client
                    .client
                    .response_code_metrics_api()
                    .create_response_code_metrics(
                        workspace_id,
                        application_id,
                        deployment_id,
                        req_body,
                    )
                    .await
            });
        }
        let results = req_waiter.join_all().await;
        let result: std::result::Result<Vec<CreateResponseCodeMetricsSuccess>, _> =
            results.into_iter().collect();
        result
            .map(|_| ())
            .map_err(|err| miette!("Error uploading observation: {err}"))
            .inspect(|_| trace!("Uploading observation to backend"))
    }

    /// Return information about the workspace given its name.
    pub(crate) async fn get_workspace_by_name(&self, name: &str) -> Result<WorkspaceSummary> {
        self.is_authenicated()?;

        trace!("Getting workspace id using its name");
        let mut workspaces: Vec<_> = self
            .client
            .workspaces_api()
            .list_workspaces(Some(name))
            .await
            .into_diagnostic()?
            .workspaces
            .into_iter()
            .filter(|workspace| workspace.display_name == name)
            .collect();

        if workspaces.len() > 1 {
            bail!("More than one workspace with the given name found.");
        } else if workspaces.len() < 1 {
            bail!("No workspace with the given name exists for this account");
        } else {
            // TODO: We can simplify this code with .ok_or()
            trace!("Successfully acquired the workspace id");
            Ok(workspaces.pop().unwrap())
        }
    }

    // TODO: Use a query parameter instead to return fewer results
    //       isntead of having to filter by name.
    /// Given the id of the workspace containing the application, and the application's
    /// name, fetch the application's information.
    pub(crate) async fn get_application_by_name(
        &self,
        workspace_id: WorkspaceId,
        name: &str,
    ) -> Result<ApplicationDetails> {
        self.is_authenicated()?;
        trace!("Getting application id using its name");

        let mut applications: Vec<_> = self
            .client
            .applications_api()
            .list_applications(workspace_id)
            .await
            .into_diagnostic()?
            .applications
            .into_iter()
            .filter(|elem| elem.display_name == name)
            .collect();

        let application = if applications.len() > 1 {
            bail!("More than one application with the given name found.");
        } else if applications.len() < 1 {
            bail!("No application with the given name exists for this account");
        } else {
            // TODO: We can simplify this code with .ok_or()
            applications.pop().unwrap()
        };

        self.client
            .applications_api()
            .get_application(workspace_id, application.id)
            .await
            .map(|success| *success.application)
            .into_diagnostic()
            .inspect(|_| trace!("Successfully acquired the workspace id"))
    }
}

/// A parsed and configured set of adapters for interacting
/// with external systems.
pub struct ApplicationConfig {
    pub platform: BoxedPlatform,
    pub ingress: BoxedIngress,
    pub monitor: BoxedMonitor,
}

#[derive(Clone)]
pub(super) struct BackendConfig {
    // TODO: Add configuration for a timeout.
    // TODO: Add a way to update the access token.
    conf: Configuration,
}

impl BackendConfig {
    pub fn new<T: AsRef<str>>(origin: Option<T>, session: Option<Session>) -> Self {
        // • Convert the Option<T> to a String.
        let origin = origin.map(|val| val.as_ref().to_owned());
        // • Set up the default configuration values.
        let jwt = session.and_then(|session| match session {
            Session::User(creds) => Some(creds.jwt),
        });
        let conf = Configuration {
            base_path: origin.unwrap_or("https://api.multitool.run".to_string()),
            bearer_access_token: jwt,
            ..Configuration::default()
        };
        Self { conf }
    }
}

impl From<BackendConfig> for Configuration {
    fn from(value: BackendConfig) -> Self {
        value.conf
    }
}

impl Deref for BackendConfig {
    type Target = Configuration;

    fn deref(&self) -> &Self::Target {
        &self.conf
    }
}

/// Add a convertion from the response type into our internal type.
impl From<LoginSuccess> for UserCreds {
    fn from(login: LoginSuccess) -> Self {
        UserCreds::new(
            login.user.email,
            login.user.jwt,
            DateTime::parse_from_rfc3339(&login.user.expires_at)
                .expect("Failed to parse JWT expiry date.")
                .into(),
        )
    }
}

pub mod deploy_meta;

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::BackendClient;

    // We have to ensure the client can be cloned, since its
    // used independently by different tasks. And because its
    // sent between tasks, it has to be both Send and Sync, too.
    assert_impl_all!(BackendClient: Clone, Send, Sync);
}
