use std::path::PathBuf;

use crate::adapters::backend::{ApplicationId, WorkspaceId};
use crate::adapters::{
    ApplicationConfig, DeploymentMetadata, IngressBuilder, MonitorBuilder, PlatformBuilder,
};
use crate::fs::{FileSystem, SessionFile};
use crate::subsystems::CONTROLLER_SUBSYSTEM_NAME;
use crate::{
    Cli, ControllerSubsystem, adapters::BackendClient, artifacts::LambdaZip, config::RunSubcommand,
};
use miette::Result;
use tokio::runtime::Runtime;
use tokio::time::Duration;
use tokio_graceful_shutdown::{IntoSubsystem as _, SubsystemBuilder, Toplevel};

use crate::Terminal;

/// The amount of time, in miliseconds, each subsystem has
/// to gracefully shutdown before being forcably shutdown.
const DEFAULT_SHUTDOWN_TIMEOUT: u64 = 5000;

/// Deploy the Lambda function as a canary and monitor it.
pub struct Run {
    _terminal: Terminal,
    artifact_path: PathBuf,
    workspace_name: String,
    application_name: String,
    backend: BackendClient,
}

impl Run {
    pub fn new(terminal: Terminal, cli: &Cli, args: RunSubcommand) -> Result<Self> {
        let fs = FileSystem::new().unwrap();
        let session = fs.load_file(SessionFile)?;

        let backend = BackendClient::new(cli, Some(session))?;

        Ok(Self {
            _terminal: terminal,
            backend,
            artifact_path: args.artifact_path,
            workspace_name: args.workspace,
            application_name: args.application,
        })
    }

    pub fn dispatch(self) -> Result<()> {
        dbg!("Executing the `run` command...");
        let rt = Runtime::new().unwrap();
        let _guard = rt.enter();
        rt.block_on(async {
            // First, we have to load the artifact.
            // This lets us fail fast in the case where the artifact
            // doesn't exist or we don't have permission to read the file.
            dbg!("Loading the lambda artifact...");
            let artifact = LambdaZip::load(&self.artifact_path).await?;
            // We need to convert our workspace and application names into the full workspace and application object
            dbg!("Loading workspace and application...");
            let workspace = self
                .backend
                .get_workspace_by_name(&self.workspace_name)
                .await?;
            let application = self
                .backend
                .get_application_by_name(workspace.id, &self.application_name)
                .await?;
            // Now, we have to load the application's configuration
            // from the backend. We have the name of the workspace and
            // application, but we need to look up the details.
            dbg!("Loading application conf...");
            let conf = ApplicationConfig {
                platform: PlatformBuilder::new(*application.platform, artifact)
                    .build()
                    .await,
                ingress: IngressBuilder::new(*application.ingress).build().await,
                monitor: MonitorBuilder::new(*application.monitor).build().await,
            };

            // Create a new deployment.
            let metadata = self.create_deployment(workspace.id, application.id).await?;

            // Build the ControllerSubsystem using the boxed objects.
            dbg!("Building controller...");
            let controller = ControllerSubsystem::builder()
                .backend(self.backend)
                .monitor(conf.monitor)
                .ingress(conf.ingress)
                .platform(conf.platform)
                .meta(metadata)
                .build();

            // Let's capture the shutdown signal from the OS.
            Toplevel::new(|s| async move {
                // • Start the action listener subsystem.
                s.start(SubsystemBuilder::new(
                    CONTROLLER_SUBSYSTEM_NAME,
                    controller.into_subsystem(),
                ));
            })
            .catch_signals()
            .handle_shutdown_requests(Duration::from_millis(DEFAULT_SHUTDOWN_TIMEOUT))
            .await
            .map_err(Into::into)
        })
    }

    async fn create_deployment(
        &self,
        workspace_id: WorkspaceId,
        application_id: ApplicationId,
    ) -> Result<DeploymentMetadata> {
        dbg!("Creating new deployment...");
        let deployment_id = self
            .backend
            .new_deployment(workspace_id, application_id)
            .await?;

        dbg!("Creating new deployment metadata...");
        let meta = DeploymentMetadata::builder()
            .workspace_id(workspace_id)
            .application_id(application_id)
            .deployment_id(deployment_id)
            .build();
        Ok(meta)
    }
}
