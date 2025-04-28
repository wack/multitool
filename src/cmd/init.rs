use crate::fs::SessionFile;
use crate::manifest::Manifest;
use crate::terminal::SelectWorkspaceOrNewOutput;
use crate::{adapters::BackendClient, fs::Session};
use miette::Result;
use tokio::runtime::Runtime;
use tracing::info;

use crate::{Terminal, config::InitSubcommand, fs::FileSystem};

/// Initialize a new project, or prepare the configuration of an existing one.
pub struct Init {
    terminal: Terminal,
    backend: BackendClient,
}

impl Init {
    pub fn new(terminal: Terminal, flags: InitSubcommand) -> Result<Self> {
        let origin = flags.origin().as_deref();
        let backend = BackendClient::new(origin, None)?;

        Ok(Self { terminal, backend })
    }

    pub fn dispatch(self) -> Result<()> {
        let rt = Runtime::new().unwrap();
        let _guard = rt.enter();
        rt.block_on(async {
            let fs = FileSystem::new()?;
            let session = self.session_or_login(&fs).await?;
            // Now that they're logged in, let's look for their
            // manifest file.
            let manifest = self.load_manifest_or_init(&fs).await;
            Ok(())
        })
    }

    /// Return the current session, or log the user in if there
    /// isn't one.
    /// # Errors
    /// This function errors if the backend can't be reached, or if the
    /// username and password are incorrect.
    async fn session_or_login(&self, fs: &FileSystem) -> Result<Session> {
        // • Load the user's session if one exists.
        match fs.load_file(SessionFile) {
            Ok(session) => Ok(session),
            Err(_) => self.prompt_and_login(fs).await,
        }
    }

    async fn prompt_and_login(&self, fs: &FileSystem) -> Result<Session> {
        info!("It appears you're not logged in. First, let's log you into your MultiTool account.");
        let email = self.terminal.prompt_email();
        let password = self.terminal.prompt_password();
        let creds = self.backend.exchange_creds(&email, &password).await?;
        fs.save_file(&SessionFile, &creds)?;
        self.terminal.logout_successful()?;
        Ok(creds)
    }

    async fn load_manifest_or_init(&self, fs: &FileSystem) -> Result<Manifest> {
        // • Attempt to read the manifest file.
        //   If no manifest is found, prompt the user to enter the
        //   configuration details.
        match fs.project_manifest() {
            Ok(manifest) => self.prompt_manifest_fields(manifest).await,
            Err(_) => {
                info!("No project manifest was found. Let's create one!");
                self.prompt_manifest().await
            }
        }
    }

    /// This function builds a manifest file field-by-field, prompting
    /// the user to initialize each one.
    async fn prompt_manifest(&self) -> Result<Manifest> {
        // • Prompt for their workspace.
        //   They should be able to select an existing workspace,
        //   or create a new one.
        let workspace = self.prompt_workspace_on_new().await?;
        println!("Workspace is ...{workspace}");
        // • Prompt for their application name. They should be able to
        //   select an existing workspace or create a new one.

        // • Prompt for their cloud provider. Right now, they can only
        //   select Amazon, but in the future we will add support for CloudFlare.
        todo!();
    }

    // When the user doesn't have a manifest file.
    async fn prompt_workspace_on_new(&self) -> Result<String> {
        info!("First, we'll choose a workspace to use.");
        let existing_workspaces = self.backend.list_workspaces().await?;
        if existing_workspaces.is_empty() {
            println!("Hit the if branch.");
            info!("Your account doesn't have any existing workspaces, so we'll create a new one.");
            info!("What would you like to name your workspace?");
            return Ok(self.terminal.prompt_new_workspace());
        } else {
            println!("Hit the else branch.");
            let choice = self
                .terminal
                .prompt_select_workspace_or_new(existing_workspaces);
            match choice {
                SelectWorkspaceOrNewOutput::ExistingWorkspace(name) => return Ok(name),
                SelectWorkspaceOrNewOutput::NewWorkspace => {
                    return Ok(self.terminal.prompt_new_workspace());
                }
            }
        }
    }

    /// The user has a project manifest file, but it may be partially
    /// uninitialized, or they may wish to update some entries. This function
    /// walks through each manifest field and asks them to fill it in or
    /// update it.
    async fn prompt_manifest_fields(&self, manifest: Manifest) -> Result<Manifest> {
        // TODO: include a log line that indicates the absolute file path
        // to the manifest file we've loaded.
        todo!();
    }
}
