use dialoguer::{Input, Password, Select};
use logging::setup_logger;
use miette::{GraphicalReportHandler, GraphicalTheme, IntoDiagnostic, Result};

use crate::Cli;

use dest::TermDestination;

mod dest;
mod logging;
mod theme;

pub(crate) use logging::{LogRouteGuard, route_logs};

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
        setup_logger(*cli.log_level());

        Self { stdout, stderr }
    }

    /// Sets the global error handler for Miette. This should be called
    /// close to `main`.
    ///
    /// Errors always render through miette's graphical handler (message,
    /// labeled source snippet, help). Only its theme follows the user's
    /// color preference for stderr: colored when allowed, plain Unicode
    /// otherwise. (miette's `DebugReportHandler` is not a "plain" mode — it
    /// prints the raw `Diagnostic { .. }` struct plus a note to enable the
    /// `fancy` feature, which this crate already enables.)
    pub fn set_error_hook(&self) -> Result<()> {
        let allow_color = self.stderr.allow_color();
        // Set the hook and coerce the `InstallError` into an `ErrorReport`
        miette::set_hook(Box::new(move |_| {
            // TODO: Add brand colors to the colored theme.
            let theme = if allow_color {
                GraphicalTheme::unicode()
            } else {
                GraphicalTheme::unicode_nocolor()
            };
            Box::new(GraphicalReportHandler::new_themed(theme))
        }))?;

        Ok(())
    }

    // TODO(@RM): Not implemented yet.
    pub fn init_check(&self) -> Result<()> {
        self.stdout
            .term()
            .write_line("Checking if the application is already initialized")
            .into_diagnostic()
    }

    pub fn init_successful(&self) -> Result<()> {
        self.stdout
            .term()
            .write_line("Package initialized successfully.")
            .into_diagnostic()
    }

    /// Write a single line to stdout. Used by the `multi check` reporting phase,
    /// which applies its own `console` styling before calling this.
    pub fn write_stdout_line(&self, line: &str) -> Result<()> {
        self.stdout.term().write_line(line).into_diagnostic()
    }

    /// Whether stdout may emit color, honoring the global `--enable-colors` flag.
    pub fn stdout_allows_color(&self) -> bool {
        self.stdout.allow_color()
    }

    /// Emit a deprecation notice for a legacy subcommand to stderr.
    ///
    /// MultiTool now centers on `multi check`; the remaining legacy subcommands
    /// are soft-deprecated (MULTI-1365) and still function, but invoking one
    /// surfaces this warning. Routed through stderr so it never pollutes a
    /// command's stdout, and honors the global `--enable-colors` setting.
    pub fn deprecation_notice(&self, command: &str) -> Result<()> {
        let body = format!(
            "warning: `multi {command}` is deprecated and will be removed in a future release. \
             MultiTool now centers on `multi check`."
        );
        let line = if self.stderr.allow_color() {
            console::style(body).yellow().to_string()
        } else {
            body
        };
        self.stderr.term().write_line(&line).into_diagnostic()
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
            .write_line("Login successful!")
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

    pub fn prompt_workspace_selection(&self, items: &[String]) -> usize {
        Select::with_theme(self.stdout.theme())
            .items(items)
            .with_prompt("Workspace")
            .interact()
            .unwrap()
    }

    pub fn prompt_application_selection(&self, items: &[String]) -> usize {
        Select::with_theme(self.stdout.theme())
            .items(items)
            .with_prompt("Application")
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

    pub fn prompt_workspace_name(&self) -> String {
        Input::with_theme(self.stdout.theme())
            .with_prompt("Workspace name")
            .interact()
            .unwrap()
    }

    pub fn prompt_application_name(&self) -> String {
        Input::with_theme(self.stdout.theme())
            .with_prompt("Application name")
            .interact()
            .unwrap()
    }
}
