use std::ops::Deref;
use std::sync::Arc;

use super::{
    BoxedIngress, BoxedMonitor, BoxedPlatform, IngressBuilder, MonitorBuilder, PlatformBuilder,
    StatusCode,
};
use crate::Cli;
use crate::fs::UserCreds;
use crate::{artifacts::LambdaZip, fs::Session, metrics::ResponseStatusCode};
use miette::miette;
use miette::{IntoDiagnostic, Result, bail};
use multitool_sdk::apis::{Api, ApiClient, configuration::Configuration};
use multitool_sdk::models::DeploymentState;
use multitool_sdk::models::{
    ApplicationDetails, ApplicationGroup, CreateResponseCodeMetricsRequest,
    CreateResponseCodeMetricsSuccess, LoginRequest, LoginSuccess, WorkspaceSummary,
};
use tokio::task::JoinSet;
use uuid::Uuid;

pub(crate) use deploy_meta::*;

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
    // TODO: Add a method for updating the access token.
}

impl Clone for BackendClient {
    fn clone(&self) -> Self {
        let conf = self.conf.clone();
        Self {
            conf: conf.clone(),
            client: ApiClient::new(Arc::new(conf)),
        }
    }
}

impl BackendClient {
    /// Return a new backend client for the MultiTool backend.
    pub fn new(cli: &Cli) -> Self {
        let conf = BackendConfig::from(cli);
        let raw_conf: Configuration = conf.clone().into();
        let client = ApiClient::new(Arc::new(raw_conf.clone()));
        Self {
            conf: raw_conf,
            client,
        }
    }

    pub(crate) async fn lock_state(
        &self,
        _meta: &DeploymentMetadata,
        _state: &DeploymentState,
    ) -> Result<LockedState> {
        // make a request to the backend to lock this particular
        // state, then return the lease expiration time.
        todo!()
    }

    pub(crate) async fn refresh_lock(
        &self,
        _meta: &DeploymentMetadata,
        _state: &LockedState,
    ) -> Result<LockedState> {
        // make a request to the backend to lock this particular
        // state, then return the lease expiration time.
        todo!()
    }

    /// Release the lock on this state without completing it.
    pub(crate) async fn abandon_lock(
        &self,
        _meta: &DeploymentMetadata,
        _state: &LockedState,
    ) -> Result<()> {
        todo!()
    }

    /// Poll the backend for in-progress states that have not yet been
    /// locked/claimed.
    pub(crate) async fn poll_for_state(
        &self,
        _meta: &DeploymentMetadata,
    ) -> Result<Vec<DeploymentState>> {
        todo!()
    }

    pub(crate) async fn mark_state_completed(
        &self,
        _meta: &DeploymentMetadata,
        _state: &LockedState,
    ) -> Result<()> {
        // This state has been effected, so the lock
        // can be released.
        todo!()
    }

    pub async fn new_deployment(
        &self,
        workspace_id: WorkspaceId,
        application_id: ApplicationId,
    ) -> Result<DeploymentId> {
        let response = self
            .client
            .deployments_api()
            .create_deployment(
                workspace_id.to_string().as_ref(),
                application_id.to_string().as_ref(),
            )
            .await
            .into_diagnostic()?;
        Ok(response.deployment.id)
    }

    /// Given the workspace name and the application name, fetch
    /// the configuration of the application.
    pub async fn fetch_config(
        &self,
        workspace: &str,
        application_name: &str,
        artifact: LambdaZip,
    ) -> Result<ApplicationConfig> {
        // • First, we have to exchange the workspace name for it's id.
        let workspace = self.get_workspace_by_name(workspace).await?;
        // • Then, we can do the same with the application name.
        let application = self
            .get_application_by_name(workspace.id, application_name)
            .await?;
        let ingress_conf = *application.ingress;
        let platform_conf = *application.platform;
        let monitor_conf = *application.monitor;
        Ok(ApplicationConfig {
            platform: PlatformBuilder::new(platform_conf, artifact).build().await,
            ingress: IngressBuilder::new(ingress_conf).build().await,
            monitor: MonitorBuilder::new(monitor_conf).build().await,
        })
    }

    /// This fuction logs the user into the backend by exchanging these credentials
    /// with the backend server.
    pub async fn exchange_creds(&self, email: &str, password: &str) -> Result<Session> {
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

        Ok(Session::User(creds))
    }

    /// Upload a batch of observations to the backend.
    pub(crate) async fn upload_observations(
        &self,
        meta: &DeploymentMetadata,
        data: Vec<StatusCode>,
    ) -> Result<()> {
        let mut req_waiter = JoinSet::new();

        for item in data {
            let group = match item.group() {
                crate::stats::Group::Control => ApplicationGroup::Baseline,
                crate::stats::Group::Experimental => ApplicationGroup::Canary,
            };
            let req_body = CreateResponseCodeMetricsRequest {
                app_class: group,
                status_2xx_count: item.get_count(&ResponseStatusCode::_2XX) as i32,
                status_4xx_count: item.get_count(&ResponseStatusCode::_4XX) as i32,
                status_5xx_count: item.get_count(&ResponseStatusCode::_5XX) as i32,
            };
            let workspace_id = meta.workspace_id().to_string();
            let application_id = meta.application_id().to_string();
            let deployment_id = *meta.deployment_id();
            let cloned_client = self.clone();
            req_waiter.spawn_local(async move {
                cloned_client
                    .client
                    .response_code_metrics_api()
                    .create_response_code_metrics(
                        &workspace_id,
                        &application_id,
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
    }

    /// Return information about the workspace given its name.
    async fn get_workspace_by_name(&self, name: &str) -> Result<WorkspaceSummary> {
        // let mut workspaces = list_workspaces(&self.conf, Some(name))
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
            Ok(workspaces.pop().unwrap())
        }
    }

    // TODO: Use a query parameter instead to return fewer results
    //       isntead of having to filter by name.
    /// Given the id of the workspace containing the application, and the application's
    /// name, fetch the application's information.
    async fn get_application_by_name(
        &self,
        workspace_id: Uuid,
        name: &str,
    ) -> Result<ApplicationDetails> {
        let mut applications: Vec<_> = self
            .client
            .applications_api()
            .list_applications(workspace_id.to_string().as_ref())
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
            .get_application(
                workspace_id.to_string().as_ref(),
                application.id.to_string().as_ref(),
            )
            .await
            .map(|success| *success.application)
            .into_diagnostic()
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

impl From<&Cli> for BackendConfig {
    fn from(cli: &Cli) -> Self {
        Self::new(cli.origin())
    }
}

impl BackendConfig {
    pub fn new<T: AsRef<str>>(origin: Option<T>) -> Self {
        // • Convert the Option<T> to a String.
        let origin = origin.map(|val| val.as_ref().to_owned());
        // • Set up the default configuration values.
        let mut conf = Configuration {
            ..Configuration::default()
        };
        // • Override the default origin.
        if let Some(origin) = origin {
            conf.base_path = origin;
        }
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
        UserCreds::new(login.user.email, login.user.jwt)
    }
}

mod deploy_meta;

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::BackendClient;

    // We have to ensure the client can be cloned, since its
    // used independently by different tasks. And because its
    // sent between tasks, it has to be both Send and Sync, too.
    assert_impl_all!(BackendClient: Clone, Send, Sync);
}
