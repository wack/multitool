use miette::{IntoDiagnostic, Result, bail};
use openapi::apis::applications_api::{get_application, list_applications};
use openapi::apis::users_api::login;
use openapi::apis::workspaces_api::list_workspaces;
use openapi::models::{ApplicationDetails, LoginRequest, WorkspaceSummary};
use uuid::Uuid;

use super::{BoxedIngress, BoxedMonitor, BoxedPlatform};
use crate::Cli;
use crate::fs::UserCreds;

use config::BackendConfig;

use crate::{
    artifacts::LambdaZip, fs::Session, metrics::ResponseStatusCode, stats::CategoricalObservation,
};

mod config;

/// The BackendClient sends requests to the MultiTool SaaS
/// backend. It wraps our generated HTTP bindings.
#[derive(Clone)]
pub struct BackendClient {
    /// We keep a copy of the OpenAPI config, which is used
    /// in each request.
    conf: BackendConfig,
    // TODO: Add a method for updating the access token.
}

impl BackendClient {
    /// Return a new backend client for the MultiTool backend.
    pub fn new(cli: &Cli) -> Self {
        let conf = BackendConfig::from(cli);
        Self { conf }
    }

    /// Given the workspace name and the application name, fetch
    /// the configuration of the application.
    pub async fn fetch_config(
        &self,
        workspace: &str,
        application: &str,
        artifact: LambdaZip,
    ) -> Result<ApplicationConfig> {
        // • First, we have to exchange the workspace name for it's id.
        let workspace = self.get_workspace_by_name(workspace).await?;
        // • Then, we can do the same with the application name.
        let _application = self
            .get_application_by_name(workspace.id, application)
            .await?;
        todo!(
            "I haven't bothered implementing the code to configure the Ingress from the configuration data"
        )
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
        let creds: UserCreds = login(&self.conf, req).await.into_diagnostic()?.into();

        Ok(Session::User(creds))
    }

    /// Upload a batch of observations to the backend.
    pub async fn upload_observations(&self, data: Vec<()>) -> Result<()> {
        todo!();
    }

    /// Return information about the workspace given its name.
    async fn get_workspace_by_name(&self, name: &str) -> Result<WorkspaceSummary> {
        let mut workspaces = list_workspaces(&self.conf, Some(name))
            .await
            .into_diagnostic()?
            .workspaces;

        if workspaces.len() > 1 {
            bail!("More than one workspace with the given name found.");
        } else if workspaces.len() < 1 {
            bail!("No workspace with the given name exists for this account");
        } else {
            // TODO: We can simplify this code with .ok_or()
            Ok(workspaces.pop().unwrap())
        }
    }

    /// Given the id of the workspace containing the application, and the application's
    /// name, fetch the application's information.
    async fn get_application_by_name(
        &self,
        workspace_id: Uuid,
        name: &str,
    ) -> Result<ApplicationDetails> {
        let mut applications: Vec<_> =
            list_applications(&self.conf, workspace_id.to_string().as_ref())
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

        get_application(
            &self.conf,
            workspace_id.to_string().as_ref(),
            application.id.to_string().as_ref(),
        )
        .await
        .into_diagnostic()
    }
}

/// A parsed and configured set of adapters for interacting
/// with external systems.
pub struct ApplicationConfig {
    pub platform: BoxedPlatform,
    pub ingress: BoxedIngress,
    pub monitor: BoxedMonitor<CategoricalObservation<5, ResponseStatusCode>>,
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::BackendClient;

    // We have to ensure the client can be cloned, since its
    // used independently by different tasks. And because its
    // sent between tasks, it has to be both Send and Sync, too.
    assert_impl_all!(BackendClient: Clone, Send, Sync);
}
