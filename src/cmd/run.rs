use std::path::PathBuf;
use std::sync::Arc;

use crate::adapters::ApplicationConfig;
use crate::subsystems::CONTROLLER_SUBSYSTEM_NAME;
use crate::{
    Cli, ControllerSubsystem, adapters::BackendClient, artifacts::LambdaZip, config::RunSubcommand,
};
use miette::Result;
use tokio::time::Duration;
use tokio_graceful_shutdown::{IntoSubsystem as _, SubsystemBuilder, Toplevel};

use crate::Terminal;

/// The amount of time, in miliseconds, each subsystem has
/// to gracefully shutdown before being forcably shutdown.
const DEFAULT_SHUTDOWN_TIMEOUT: u64 = 5000;

/// Deploy the Lambda function as a canary and monitor it.
pub struct Run {
    terminal: Terminal,
    artifact_path: PathBuf,
    workspace: String,
    application: String,
    backend: BackendClient,
}

impl Run {
    pub fn new(terminal: Terminal, cli: &Cli, args: RunSubcommand) -> Self {
        let backend = BackendClient::new(cli);
        Self {
            terminal,
            backend,
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
        let conf = self
            .backend
            .fetch_config(&self.workspace, &self.application, artifact)
            .await?;
        let ApplicationConfig {
            ingress,
            platform,
            monitor,
        } = conf;
        // Build the ControllerSubsystem using the boxed objects.
        let controller = ControllerSubsystem::new(self.backend, monitor, ingress, platform);
        //   …but before we do, let's capture the shutdown
        //   signal from the OS.
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
    }
}
