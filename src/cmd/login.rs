use crate::adapters::{BackendClient, MultiToolBackend};
use crate::Flags;
use miette::Result;

use crate::{config::LoginSubcommand, fs::FileSystem, Terminal};

/// Deploy the Lambda function as a canary and monitor it.
pub struct Login {
    terminal: Terminal,
    flags: LoginSubcommand,
    backend: Box<dyn BackendClient>,
}

impl Login {
    pub fn new(terminal: Terminal, login_flags: LoginSubcommand, flags: &Flags) -> Self {
        let backend = Box::new(MultiToolBackend::new(flags));
        Self {
            terminal,
            flags: login_flags,
            backend,
        }
    }

    pub async fn dispatch(self) -> Result<()> {
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

        let creds = self.backend.exchange_creds(email, password).await?;

        // • Save the auth credentials to disk.
        fs.save_file(&creds, &creds)?;

        // • Print a success message.
        self.terminal.login_successful()
    }
}
