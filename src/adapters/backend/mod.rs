use async_trait::async_trait;
use miette::{bail, IntoDiagnostic, Result};
use openapi::apis::applications_api::{get_application, list_applications};
use openapi::apis::configuration::Configuration;
use openapi::apis::workspaces_api::{self, list_workspaces};
use openapi::models::{
    ApplicationConfig, ApplicationDetails, ApplicationSummary, WorkspaceSummary,
};
use uuid::Uuid;

/// Backend references the MultiTool backend.
#[async_trait]
pub trait BackendClient {
    /// Given the workspace name and the application name, fetch
    /// the configuration of the application.
    async fn fetch_config(workspace: String, application: String) -> Result<()>;

    // TODO: Add Login.
}

pub struct MultiToolBackend {
    conf: Configuration,
}

#[async_trait]
impl BackendClient for MultiToolBackend {
    async fn fetch_config(workspace: String, application: String) -> Result<()> {
        todo!()
    }
}

impl MultiToolBackend {
    /// Return a new backend client for the MultiTool backend.
    pub fn new(origin: Option<String>) -> Self {
        let mut conf = Configuration {
            ..Configuration::default()
        };

        if let Some(origin) = origin {
            conf.base_path = origin;
        }

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
