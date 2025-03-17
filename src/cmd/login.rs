use crate::adapters::BackendClient;
use crate::{Cli, fs::SessionFile};
use miette::Result;
use tokio::runtime::Runtime;

use crate::{Terminal, config::LoginSubcommand, fs::FileSystem};

/// Deploy the Lambda function as a canary and monitor it.
pub struct Login {
    terminal: Terminal,
    flags: LoginSubcommand,
    backend: BackendClient,
}

impl Login {
    pub fn new(terminal: Terminal, cli: &Cli, flags: LoginSubcommand) -> Result<Self> {
        let fs = FileSystem::new().unwrap();
        let session = fs.load_file(SessionFile)?;

        let backend = BackendClient::new(cli, session)?;

        Ok(Self {
            terminal,
            flags,
            backend,
        })
    }

    pub fn dispatch(self) -> Result<()> {
        let rt = Runtime::new().unwrap();
        let _guard = rt.enter();
        rt.block_on(async {
            let fs = FileSystem::new()?;
            // If no username was provided, prompt for their username.
            let email = self
                .flags
                .email()
                .map(ToString::to_string)
                .unwrap_or_else(|| self.terminal.prompt_email());

            // If no password was provided, prompt for their password.
            let password = self
                .flags
                .password()
                .map(ToString::to_string)
                .unwrap_or_else(|| self.terminal.prompt_password());

            let creds = self.backend.exchange_creds(&email, &password).await?;

            // • Save the auth credentials to disk.
            fs.save_file(&SessionFile, &creds)?;

            // • Print a success message.
            self.terminal.login_successful()
        })
    }
}
