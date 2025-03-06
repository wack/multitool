use clap::Subcommand;
use miette::Result;

use crate::cmd::{Login, Logout, Run, Version};
use crate::terminal::Terminal;

use super::{Cli, LoginSubcommand, RunSubcommand};

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
    pub async fn dispatch(self, console: Terminal, cli: &Cli) -> Result<()> {
        match self {
            Self::Login(flags) => Login::new(console, cli, flags)?.dispatch().await,
            Self::Logout => Logout::new(console).dispatch(),
            Self::Run(flags) => Run::new(console, cli, flags).dispatch().await,
            Self::Version => Version::new(console).dispatch(),
        }
    }
}
