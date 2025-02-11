use dialoguer::{Input, Password};
use miette::{DebugReportHandler, GraphicalReportHandler, IntoDiagnostic, Result};

use crate::Cli;

use dest::TermDestination;

mod dest;
mod theme;

pub struct Terminal {
    stdout: TermDestination,
    stderr: TermDestination,
}

impl Terminal {
    pub fn new(cli: &Cli) -> Self {
        // Check to see whether we should color the
        // terminal output.
        let stdout = TermDestination::stdout(cli);
        let stderr = TermDestination::stderr(cli);

        Self { stdout, stderr }
    }

    /// This constructs and sets the global error reporter we use -- constructed during
    /// initialization, this handler is graphical if the user allows
    /// terminal colors, and simple otherwise.
    /// This field is used to set the error reporting hook.
    /// Sets the global error handler for Miette. This should be called
    /// close to `main`.
    pub fn set_error_hook(&self) -> Result<()> {
        let allow_color = self.stderr.allow_color();
        // Set the hook and coerce the `InstallError` into an `ErrorReport`
        miette::set_hook(Box::new(move |_| {
            if allow_color {
                // TODO: Add brand colors using ``::new_themed()`
                Box::new(GraphicalReportHandler::new())
            } else {
                Box::new(DebugReportHandler)
            }
        }))?;

        Ok(())
    }

    // TODO(@RM): Not implemented yet.
    pub fn init_check(&self) -> Result<()> {
        self.stdout
            .term()
            .write_line("Checking if the project is already initialized")
            .into_diagnostic()
    }

    pub fn init_successful(&self) -> Result<()> {
        self.stdout
            .term()
            .write_line("Package initialized successfully.")
            .into_diagnostic()
    }

    pub fn print_version(&self, version: &'static str) -> Result<()> {
        let msg = format!("v{version}");
        self.stdout
            .term()
            .write_line(msg.as_str())
            .into_diagnostic()
    }

    pub fn logout_successful(&self) -> Result<()> {
        self.stdout
            .term()
            .write_line("Logout successful.")
            .into_diagnostic()
    }

    pub fn login_successful(&self) -> Result<()> {
        self.stdout
            .term()
            .write_line("Logout successful.")
            .into_diagnostic()
    }

    pub fn account_create_successful(&self) -> Result<()> {
        self.stdout
            .term()
            .write_line("Account created successfully! Check your email for a verification link.")
            .into_diagnostic()
    }

    pub fn prompt_email(&self) -> String {
        Input::with_theme(self.stdout.theme())
            .with_prompt("Email")
            .interact()
            .unwrap()
    }

    // TODO: Use a secure string to ensure password safety.
    pub fn prompt_password(&self) -> String {
        Password::with_theme(self.stdout.theme())
            .with_prompt("Password")
            .interact()
            .unwrap()
    }
}
