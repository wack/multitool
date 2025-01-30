use crate::fs::UserCreds;
use miette::{IntoDiagnostic, Result};
use openapi::apis::{configuration::Configuration, login_api::login};
use openapi::models::LoginRequest;

use crate::{
    config::LoginFlags,
    fs::{FileSystem, Session},
    Terminal,
};

/// Deploy the Lambda function as a canary and monitor it.
pub struct Login {
    terminal: Terminal,
    flags: LoginFlags,
}

impl Login {
    pub fn new(terminal: Terminal, flags: LoginFlags) -> Self {
        Self { terminal, flags }
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

        // TODO: We also need to return an "expires at" timestamp
        //       so we can expire the token.
        let creds = Self::exchange_creds(email, password).await?;

        // • Save the auth credentials to disk.
        fs.save_file(&creds, &creds)?;

        // • Print a success message.
        self.terminal.login_successful()
    }

    /// Exchange auth credentials with the server for an auth token.
    /// Account is either the user's account name or email address.
    async fn exchange_creds(email: String, password: String) -> Result<Session> {
        // TODO: Create the user's configuration higher up the call stack
        // and globally reuse it, where possible.
        let conf = Configuration {
            ..Configuration::default()
        };
        // • Create and send the request, marshalling the result
        //   into user credentials.
        let creds: UserCreds = login(&conf, LoginRequest { email, password })
            .await
            .into_diagnostic()?
            .into();

        Ok(Session::User(creds))
    }
}
