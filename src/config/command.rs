use clap::Subcommand;
use miette::Result;

use crate::cmd::{Login, Logout, Run, Version};
use crate::terminal::Terminal;

use super::{Flags, LoginSubcommand, RunSubcommand};

/// A `MultiCommand` is one of the top-level commands accepted by
/// the multi CLI.
#[derive(Subcommand, Clone)]
pub enum MultiCommand {
    /// Log in to the hosted SaaS.
    Login(LoginSubcommand),
    Logout,
    /// Run will execute `multi` in "runner mode", where it will
    /// immediately deploy the provided artifact and start canarying.
    Run(RunSubcommand),
    /// Print the CLI version and exit
    Version,
}

impl MultiCommand {
    /// dispatch the user-provided arguments to the command handler.
    pub async fn dispatch(self, console: Terminal, flags: &Flags) -> Result<()> {
        match self {
            Self::Login(login_flags) => Login::new(console, login_flags, flags).dispatch().await,
            Self::Logout => Logout::new(console).dispatch(),
            Self::Run(run_flags) => Run::new(console, run_flags).dispatch().await,
            Self::Version => Version::new(console).dispatch(),
        }
    }
}
