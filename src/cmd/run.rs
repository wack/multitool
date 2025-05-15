use std::path::PathBuf;

use crate::adapters::backend::{ApplicationId, WorkspaceId};
use crate::adapters::{
    ApplicationConfig, IngressBuilder, MonitorBuilder, PlatformBuilder, RolloutMetadata,
};
use crate::fs::{FileSystem, SessionFile, project_manifest};
use crate::subsystems::CONTROLLER_SUBSYSTEM_NAME;
use crate::{
    ControllerSubsystem, adapters::BackendClient, artifacts::LambdaZip, config::RunSubcommand,
};
use miette::{Diagnostic, Result};
use thiserror::Error;
use tokio::runtime::Runtime;
use tokio::time::Duration;
use tokio_graceful_shutdown::{IntoSubsystem as _, SubsystemBuilder, Toplevel};
use tracing::{debug, info};

use crate::Terminal;

/// The amount of time, in miliseconds, each subsystem has
/// to gracefully shutdown before being forcably shutdown.
const DEFAULT_SHUTDOWN_TIMEOUT: u64 = 5000;

/// Deploy the Lambda function as a canary and monitor it.
pub struct Run {
    _terminal: Terminal,
    manifest: Manifest,
    artifact_path: PathBuf,
    override_workspace_name: Option<String>,
    override_application_name: Option<String>,
    backend: BackendClient,
}

#[derive(Error, Debug, Diagnostic)]
#[error(
    "No workspace name found. You must provide the target workspace name, either using the $MULTI_WORKSPACE environment variable, the --workspace flag, or setting it in your config file"
)]
struct MissingWorkspace;

#[derive(Error, Debug, Diagnostic)]
#[error(
    "No aplication name found. You must provide the target application name, either using the $MULTI_WORKSPACE environment variable, the --workspace flag, or setting it in your config file"
)]
struct MissingApplication;

impl Run {
    pub fn new(terminal: Terminal, mut args: RunSubcommand) -> Result<Self> {
        let fs = FileSystem::new().unwrap();
        let session = fs.load_file(SessionFile)?;
        let manifest = project_manifest().clone();
        let backend = BackendClient::new(args.origin(), Some(session))?;

        Ok(Self {
            _terminal: terminal,
            manifest,
            backend,
            artifact_path: args.artifact_path().as_ref().to_owned(),
            override_workspace_name: args.workspace().clone(),
            override_application_name: args.application().clone(),
        })
    }

    fn application_name(&self) -> Result<&str> {
        self.override_application_name
            .as_deref()
            .or(self.manifest.application.as_deref())
            .ok_or(MissingApplication)
    }

    pub fn dispatch(self) -> Result<()> {
        info!("Starting MultiTool!");
        let rt = Runtime::new().unwrap();
        let _guard = rt.enter();
        rt.block_on(async {
            // First, we have to load the artifact.
            // This lets us fail fast in the case where the artifact
            // doesn't exist or we don't have permission to read the file.
            debug!("Loading the lambda artifact...");
            let artifact = LambdaZip::load(&self.artifact_path).await?;
            // We need to convert our workspace and application names into the full workspace and application object
            debug!("Loading workspace and application...");
            let workspace_name = self.workspace_name()?;
            let application_name = self.application_name()?;
            let workspace = self
                .backend
                .get_workspace_by_name(workspace_name)
                .await?;
            let application = self
                .backend
                .get_application_by_name(workspace.id, application_name)
                .await?;
            // Now, we have to load the application's configuration
            // from the backend. We have the name of the workspace and
            // application, but we need to look up the details.
            debug!("Loading application conf...");
            let conf = ApplicationConfig {
                platform: PlatformBuilder::new(*application.platform, artifact)
                    .build()
                    .await,
                ingress: IngressBuilder::new(*application.ingress).build().await,
                monitor: MonitorBuilder::new(*application.monitor).build().await,
            };

            // Create a new rollout.
            let metadata = self.create_rollout(workspace.id, application.id).await?;

            // Build the ControllerSubsystem using the boxed objects.
            debug!("Building controller...");
            let controller = ControllerSubsystem::builder()
                .backend(self.backend)
                .monitor(conf.monitor)
                .ingress(conf.ingress)
                .platform(conf.platform)
                .meta(metadata)
                .build();

            info!("Starting the rollout...");

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

    async fn create_rollout(
        &self,
        workspace_id: WorkspaceId,
        application_id: ApplicationId,
    ) -> Result<RolloutMetadata> {
        debug!("Creating new rollout...");
        let rollout = self
            .backend
            .new_rollout(workspace_id, application_id)
            .await?;

        info!(
            "New rollout created! Follow along in the dashboard:\nhttps://app.multitool.run/workspaces/{}/applications/{}/activity/{}/events",
            workspace_id, application_id, rollout.number
        );

        debug!("Creating new rollout metadata...");
        let meta = RolloutMetadata::builder()
            .workspace_id(workspace_id)
            .application_id(application_id)
            .rollout_id(rollout.id)
            .build();
        Ok(meta)
    }
}

    fn application_name(&self) -> Result<&str> {
        self.override_application_name
            .as_deref()
            .or(self.manifest.application.as_deref())
            .ok_or(MissingApplication)
    }

    fn workspace_name(&self) -> Result<&str> {
        self.override_workspace_name
            .as_deref()
            .or(self.manifest.workspace.as_deref())
            .ok_or(MissingWorkspace)
    }


