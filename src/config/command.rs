use clap::Subcommand;
use miette::Result;

#[cfg(feature = "proxy")]
use crate::cmd::Proxy;
use crate::cmd::{Login, Logout, Run, Version};
use crate::terminal::Terminal;

use super::{Cli, LoginSubcommand, RunSubcommand};

#[cfg(feature = "proxy")]
use super::ProxySubcommand;

/// A `MultiCommand` is one of the top-level commands accepted by
/// the multi CLI.
#[derive(Subcommand, Clone)]
pub enum MultiCommand {
    /// Log in to the hosted SaaS.
    Login(LoginSubcommand),
    Logout,
    #[cfg(feature = "proxy")]
    Proxy(ProxySubcommand),
    /// Run will execute `multi` in "runner mode", where it will
    /// immediately deploy the provided artifact and start canarying.
    Run(RunSubcommand),
    /// Print the CLI version and exit
    Version,
}

impl MultiCommand {
    /// dispatch the user-provided arguments to the command handler.
    pub fn dispatch(self, console: Terminal, cli: &Cli) -> Result<()> {
        match self {
            Self::Login(flags) => Login::new(console, flags)?.dispatch(),
            Self::Logout => Logout::new(console).dispatch(),
            #[cfg(feature = "proxy")]
            Self::Proxy(flags) => Proxy::new(console, cli, flags).dispatch(),
            Self::Run(flags) => Run::new(console, flags)?.dispatch(),
            Self::Version => Version::new(console).dispatch(),
        }
    }
}
