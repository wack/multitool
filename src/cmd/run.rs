use std::path::PathBuf;

use crate::{artifacts::LambdaZip, config::RunSubcommand};
use miette::Result;
use openapi::apis::{configuration::Configuration, workspaces_api::list_workspaces};

use crate::Terminal;

/// Deploy the Lambda function as a canary and monitor it.
pub struct Run {
    terminal: Terminal,
    artifact_path: PathBuf,
    workspace: String,
    application: String,
}

type WorkspaceId = String;
type ApplicationConfig = String;
type ApplicationId = String;
type DeploymentId = String;

trait RunBackend {
    // Given the workspace name, return the workspace ID.
    fn query_workspace(&mut self, name: String) -> Result<WorkspaceId>;
    // Given the workspace id and the application name, return
    // the application configuration and ID.
    fn query_application(
        &mut self,
        name: String,
        workspace: WorkspaceId,
    ) -> Result<ApplicationConfig>;
    /// Create a new deployment for this application.
    fn create_deployment(&mut self, id: ApplicationId) -> Result<DeploymentId>;

    /// Check the status of the deployment.
    fn check_deployment(&mut self, id: DeploymentId);
}

impl Run {
    pub fn new(terminal: Terminal, args: RunSubcommand) -> Self {
        Self {
            terminal,
            artifact_path: args.artifact_path,
            workspace: args.workspace,
            application: args.application,
        }
    }

    pub async fn dispatch(self) -> Result<()> {
        // • First, we have to load the artifact.
        //   This lets us fail fast in the case where the artifact
        //   doesn't exist or we don't have permission to read the file.
        let artifact = LambdaZip::load(self.artifact_path).await?;
        // • Now, we have to load the application's configuration
        //   from the backend. We have the name of the workspace and
        //   application, but we need to look up the details.
        let display_name = self.workspace.clone();
        let conf = Configuration {
            ..Configuration::default()
        };
        let workspaces = list_workspaces(&conf, Some(&display_name)).await;
        todo!();
    }
}
