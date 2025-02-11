use async_trait::async_trait;
use miette::{bail, IntoDiagnostic, Result};
use openapi::apis::applications_api::{get_application, list_applications};
use openapi::apis::users_api::login;
use openapi::apis::workspaces_api::list_workspaces;
use openapi::models::{ApplicationDetails, LoginRequest, WorkspaceSummary};
use uuid::Uuid;

use crate::fs::UserCreds;
use crate::Flags;

use super::{BackendClient, BackendConfig, Session};

pub struct MultiToolBackend {
    conf: BackendConfig,
}

#[async_trait]
impl BackendClient for MultiToolBackend {
    async fn fetch_config(&self, workspace: String, application: String) -> Result<()> {
        todo!()
    }

    /// Exchange auth credentials with the server for an auth token.
    /// Account is either the user's account name or email address.
    async fn exchange_creds(&self, email: String, password: String) -> Result<Session> {
        // • Create and send the request, marshalling the result
        //   into user credentials.
        let creds: UserCreds = login(&self.conf, LoginRequest { email, password })
            .await
            .into_diagnostic()?
            .into();

        Ok(Session::User(creds))
    }
}

impl MultiToolBackend {
    /// Return a new backend client for the MultiTool backend.
    pub fn new(flags: &Flags) -> Self {
        let conf = BackendConfig::from(flags);
        Self { conf }
    }

    /// Return information about the workspace given its name.
    async fn get_workspace_by_name(&self, name: String) -> Result<WorkspaceSummary> {
        let display_name = Some(name.as_ref());
        let mut workspaces = list_workspaces(&self.conf, display_name)
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
        name: String,
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
