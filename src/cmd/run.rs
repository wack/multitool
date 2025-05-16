use std::path::PathBuf;

use crate::adapters::backend::{ApplicationId, WorkspaceId};
use crate::adapters::{
    ApplicationConfig, BoxedIngress, BoxedMonitor, BoxedPlatform, Platform, PlatformBuilder,
    RolloutMetadata,
};
use crate::fs::{FileSystem, SessionFile, project_manifest};
use crate::manifest::{CloudflareConfig, Manifest};
use crate::subsystems::CONTROLLER_SUBSYSTEM_NAME;
use crate::{
    ControllerSubsystem, adapters::BackendClient, artifacts::LambdaZip, config::RunSubcommand,
};
use miette::{Context, Diagnostic, Result, miette};
use multitool_sdk::models::{ApplicationDetails, WorkspaceSummary};
use thiserror::Error;
use tokio::join;
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
    args: RunSubcommand,
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
    pub fn new(terminal: Terminal, args: RunSubcommand) -> Result<Self> {
        let fs = FileSystem::new().unwrap();
        let session = fs.load_file(SessionFile)?;
        let manifest = project_manifest().clone();
        let backend = BackendClient::new(args.origin(), Some(session))?;
        let artifact_path = args.artifact_path().as_ref().to_owned();
        let override_workspace_name = args.workspace().map(ToString::to_string);
        let override_application_name = args.application().map(ToString::to_string);

        Ok(Self {
            args,
            _terminal: terminal,
            manifest,
            backend,
            artifact_path,
            override_workspace_name,
            override_application_name,
        })
    }

    /// Resolve precedence order:
    /// 1. CLI flag has highest precedence.
    /// 2. Environment variable.
    /// 3. Manifest value has lowest precedence.
    fn workspace_name(&self) -> Result<String> {
        let manifest_workspace = self.manifest.workspace().map(ToString::to_string);
        self.override_workspace_name
            .clone()
            .or(manifest_workspace)
            .ok_or(MissingWorkspace.into())
    }

    /// Resolve precedence order:
    /// 1. CLI flag has highest precedence.
    /// 2. Environment variable.
    /// 3. Manifest value has lowest precedence.
    fn application_name(&self) -> Result<String> {
        let manifest_application = self.manifest.application().map(ToString::to_string);
        self.override_application_name
            .clone()
            .or(manifest_application)
            .ok_or(MissingApplication.into())
    }

    async fn validate_workspace(&self, name: &str) -> Result<WorkspaceSummary> {
        // TODO: Turn this into a struct and include two hints:
        // 1. Are you logged into the right account?
        // 2. Create a new workspace (from the CLI).
        let workspace_not_found =
            miette!("The workspace {name} does not exist within your account.");
        self.backend
            .get_workspace_by_name(&name)
            .await
            .context(workspace_not_found)
    }

    async fn load_platform(&self, manifest: &Manifest) -> Result<BoxedPlatform> {
        manifest.load_platform(&self.args)
    }

    async fn load_ingress(&self, manifest: &Manifest) -> Result<BoxedIngress> {
        manifest.load_ingress(&self.args).await
    }

    async fn load_monitor(&self, manifest: &Manifest) -> Result<BoxedMonitor> {
        manifest.load_monitor(&self.args).await
    }

    async fn validate_application(
        &self,
        workspace: &WorkspaceSummary,
        name: &str,
    ) -> Result<ApplicationDetails> {
        // TODO: Turn this into a struct and include two hints:
        // 1. Are you logged into the right account?
        // 2. Create a new application (from the CLI).
        let workspace_name = &workspace.display_name;
        let application_not_found =
            miette!("The application {name} does not exist within the workspace {workspace_name}.");
        self.backend
            .get_application_by_name(workspace.id, &name)
            .await
            .context(application_not_found)
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
            // Validate that the application name and workspace name exists in this user's account.
            let workspace = self.validate_workspace(&workspace_name).await?;
            let application = self
                .validate_application(&workspace, &application_name)
                .await?;

            // Now, we have to load the application's configuration
            // from the backend. We have the name of the workspace and
            // application, but we need to look up the details.
            debug!("Loading application conf...");
            let (platform_result, ingress_result, monitor_result) = join!(
                self.load_platform(&self.manifest),
                self.load_ingress(&self.manifest),
                self.load_monitor(&self.manifest),
            );
            let (platform, ingress, monitor) = (platform_result?, ingress_result?, monitor_result?);

            // Create a new rollout.
            let metadata = self.create_rollout(workspace.id, application.id).await?;

            // Build the ControllerSubsystem using the boxed objects.
            debug!("Building controller...");
            let controller = ControllerSubsystem::builder()
                .backend(self.backend)
                .monitor(monitor)
                .ingress(ingress)
                .platform(platform)
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
