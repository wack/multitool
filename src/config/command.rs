use clap::Subcommand;
use miette::Result;

use crate::cmd::{Login, Logout, Run, Version};
use crate::terminal::Terminal;

use super::LoginFlags;

/// A `MultiCommand` is one of the top-level commands accepted by
/// the multi CLI.
#[derive(Subcommand, Clone)]
pub enum MultiCommand {
    /// Log in to the hosted SaaS.
    Login(LoginFlags),
    Logout,
    /// Run will execute `multi` in "runner mode", where it will
    /// immediately deploy the provided artifact and start canarying.
    Run,
    /// Print the CLI version and exit
    Version,
}

impl MultiCommand {
    /// dispatch the user-provided arguments to the command handler.
    pub async fn dispatch(self, console: Terminal) -> Result<()> {
        match self {
            Self::Login(flags) => Login::new(console, flags).dispatch().await,
            Self::Logout => Logout::new(console).dispatch(),
            Self::Run => Run::new(console).dispatch(),
            Self::Version => Version::new(console).dispatch(),
        }
    }
}
