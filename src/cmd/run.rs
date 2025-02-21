use std::path::PathBuf;

use crate::adapters::ApplicationConfig;
use crate::subsystems::{
    ACTION_LISTENER_SUBSYSTEM_NAME, INGRESS_SUBSYSTEM_NAME, MONITOR_SUBSYSTEM_NAME,
    PLATFORM_SUBSYSTEM_NAME,
};
use crate::{
    ActionListenerSubsystem, Cli, IngressSubsystem, MonitorSubsystem, PlatformSubsystem,
    adapters::{BackendClient, MultiToolBackend},
    artifacts::LambdaZip,
    config::RunSubcommand,
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
    backend: Box<dyn BackendClient>,
}

impl Run {
    pub fn new(terminal: Terminal, cli: &Cli, args: RunSubcommand) -> Self {
        let backend = Box::new(MultiToolBackend::new(cli));
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
        // let monitor_conf = conf.monitor().clone();
        // • Using the application configuration, we can spawn
        //   the Monitor, the Platform, and the Ingress.
        let ingress = IngressSubsystem::new(ingress);
        let monitor = MonitorSubsystem::new(monitor);
        let platform = PlatformSubsystem::new(platform);
        let listener = ActionListenerSubsystem;
        //   …but before we do, let's capture the shutdown
        //   signal from the OS.
        Toplevel::new(|s| async move {
            // • Start the monitor subsystem.
            s.start(SubsystemBuilder::new(
                MONITOR_SUBSYSTEM_NAME,
                monitor.into_subsystem(),
            ));
            // • Start the platform subsystem.
            s.start(SubsystemBuilder::new(
                PLATFORM_SUBSYSTEM_NAME,
                platform.into_subsystem(),
            ));
            // • Start the ingress subsystem.
            s.start(SubsystemBuilder::new(
                INGRESS_SUBSYSTEM_NAME,
                ingress.into_subsystem(),
            ));
            // • Start the action listener subsystem.
            s.start(SubsystemBuilder::new(
                ACTION_LISTENER_SUBSYSTEM_NAME,
                listener.into_subsystem(),
            ));
        })
        .catch_signals()
        .handle_shutdown_requests(Duration::from_millis(DEFAULT_SHUTDOWN_TIMEOUT))
        .await
        .map_err(Into::into)
    }
}
