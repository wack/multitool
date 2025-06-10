//! The `init` subcommand is implemented as a state machine
//! that maniputates stack allocated data with each state.
//! The input is a (potentially empty) manifest file. Each state
//! asks a question of the user to fill in the manifest file,
//! like 20 Questions. As you answer more and more questions, you
//! navigate deeper and deeper into the manifest's structure,
//! until you reach a leaf node. Each leaf node corresponds to a
//! field in the manifest.

use std::sync::Arc;

use async_trait::async_trait;
use bon::Builder;
use miette::{IntoDiagnostic, Result};
use tokio::runtime::Runtime;
use tracing::info;

use crate::{
    MULTITOOL_ORIGIN, Terminal,
    adapters::BackendClient,
    config::InitSubcommand,
    fs::{FileSystem, Session, SessionFile},
    manifest::{InitTomlManifest, Manifest},
};

pub struct Init {
    terminal: Terminal,
    origin: String,
}

impl Init {
    pub fn new(terminal: Terminal, flags: InitSubcommand) -> Result<Self> {
        let origin = flags
            .origin()
            .clone()
            .unwrap_or(MULTITOOL_ORIGIN.to_owned());
        Ok(Self { terminal, origin })
    }

    pub fn dispatch(self) -> Result<()> {
        // Kick off the async runtime.
        let rt = Runtime::new().into_diagnostic()?;
        let _guard = rt.enter();
        rt.block_on(async {
            let fs = FileSystem::new()?;

            // Exit early if there's already a project manifest.
            //
            // For now, we don't allow users to use init to
            // edit their manifest (because we can't preserve
            // comments right Serde right now...)
            // TODO: Upgrade our config-file reading to use
            // the toml and toml_edit crates to preserve comments
            // in the TOML files we read.
            // https://docs.rs/toml_edit/latest/toml_edit/
            if let Ok(_) = fs.project_manifest() {
                info!("It looks like you already have an initialized project manifest. Exiting.");
                return Ok(());
            }
            info!("No project manifest file found. Let's create one!");
            // Create a new manifest instance.
            let manifest = Arc::new(Manifest::default());
            let terminal = Arc::new(self.terminal);
            let start = Start::builder()
                .manifest(manifest)
                .fs(fs.clone())
                .terminal(terminal)
                .origin(self.origin.clone())
                .build();
            let mut state = State::Next(Box::new(start));

            let manifest = loop {
                match state {
                    State::Next(mut next) => state = next.run().await,
                    State::Done(manifest) => break manifest,
                    State::Err(report) => return Err(report),
                }
            };

            // Dump the file to disk.
            fs.save_file(&InitTomlManifest, &*manifest)
        })
    }
}

#[derive(Builder)]
struct Start {
    manifest: Arc<Manifest>,
    terminal: Arc<Terminal>,
    origin: String,
    fs: FileSystem,
}

#[async_trait]
impl InitStateMachine for Start {
    type Output = Arc<Manifest>;

    async fn run(&mut self) -> State<Self::Output> {
        let next = CheckLogin::builder()
            .manifest(self.manifest.clone())
            .fs(self.fs.clone())
            .terminal(self.terminal.clone())
            .origin(self.origin.clone())
            .build();
        State::Next(Box::new(next))
    }
}

#[derive(Builder)]
struct CheckLogin {
    manifest: Arc<Manifest>,
    terminal: Arc<Terminal>,
    fs: FileSystem,
    origin: String,
}

impl CheckLogin {
    async fn prompt_login(&self) -> Result<Session> {
        let email = self.terminal.prompt_email();
        let password = self.terminal.prompt_password();
        let backend = BackendClient::new(Some(&self.origin), None)?;
        let creds = backend.exchange_creds(&email, &password).await?;
        self.fs.save_file(&SessionFile, &creds)?;
        self.terminal.login_successful()?;
        info!("Now that you're logged in, we can continue.");
        Ok(creds)
    }
}

impl<T> From<miette::Report> for State<T> {
    fn from(value: miette::Report) -> Self {
        Self::Err(value)
    }
}

#[async_trait]
impl InitStateMachine for CheckLogin {
    type Output = Arc<Manifest>;

    async fn run(&mut self) -> State<Self::Output> {
        // Check to ensure the user is logged in.
        // If they're already logged in, then we can continue
        // forward with the `init` process.
        info!("Checking to see if you're logged in");
        let session = match self.fs.load_file(SessionFile) {
            Ok(session) => Ok(session),
            // If they're not logged in, then we have to log
            // them in and get back a session instance so we
            // can build the Backend client.
            Err(_) => self.prompt_login().await,
        };

        // Now that they're logged in, we can create a backend
        // client using their auth information.
        let origin = Some(self.origin.as_ref());
        let backend = match session {
            Ok(value) => BackendClient::new(origin, Some(value)),
            Err(err) => return State::Err(err),
        };
        // Unwrap the backend client.
        let backend = match backend {
            Ok(backend) => backend,
            Err(err) => return State::Err(err),
        };
        // Now, we can ask the user about which workspace they
        // want to operate on.
        let next = PromptWorkspace::builder()
            .manifest(self.manifest.clone())
            .terminal(self.terminal.clone())
            .fs(self.fs.clone())
            .backend(backend)
            .build();

        State::Next(Box::new(next))
    }
}

#[derive(Builder)]
struct PromptWorkspace {
    manifest: Arc<Manifest>,
    terminal: Arc<Terminal>,
    fs: FileSystem,
    backend: BackendClient,
}

#[async_trait]
impl InitStateMachine for PromptWorkspace {
    type Output = Arc<Manifest>;

    async fn run(&mut self) -> State<Self::Output> {
        State::Done(self.manifest.clone())
    }
}

enum State<T> {
    Done(T),
    Next(Box<dyn InitStateMachine<Output = T>>),
    Err(miette::Report),
}

#[async_trait]
trait InitStateMachine {
    type Output;
    async fn run(&mut self) -> State<Self::Output>;
}

#[cfg(test)]
mod tests {
    use crate::manifest::Manifest;

    use super::InitStateMachine;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(InitStateMachine<Output = Manifest>);
}
