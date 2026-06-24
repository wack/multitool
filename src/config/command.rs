use clap::Subcommand;
use miette::Result;

#[cfg(feature = "proxy")]
use crate::cmd::Proxy;
use crate::cmd::{Check, Init, Login, Logout, Run, Version};
use crate::terminal::Terminal;

use super::{CheckSubcommand, InitSubcommand, LoginSubcommand, RunSubcommand};

#[cfg(feature = "proxy")]
use super::ProxySubcommand;

/// A `MultiCommand` is one of the top-level commands accepted by
/// the multi CLI.
#[derive(Subcommand, Clone)]
pub enum MultiCommand {
    /// Log in to the hosted SaaS.
    Login(LoginSubcommand),
    Logout,
    /// Initialize a new application or prepare the configuration of an existing one
    #[command(hide = true)]
    Init(InitSubcommand),
    #[cfg(feature = "proxy")]
    Proxy(ProxySubcommand),
    /// Run will execute `multi` in "runner mode", where it will
    /// immediately deploy the provided artifact and start canarying.
    Run(RunSubcommand),
    /// Validate the requirements declared in `CHECKS.md` files using AI-agent checks.
    Check(CheckSubcommand),
    /// Print the CLI version and exit
    Version,
}

impl MultiCommand {
    /// dispatch the user-provided arguments to the command handler.
    pub fn dispatch(self, console: Terminal) -> Result<()> {
        match self {
            Self::Login(flags) => Login::new(console, flags)?.dispatch(),
            Self::Logout => Logout::new(console).dispatch(),
            Self::Init(flags) => Init::new(console, flags)?.dispatch(),
            #[cfg(feature = "proxy")]
            Self::Proxy(flags) => Proxy::new(console, flags).dispatch(),
            Self::Run(flags) => Run::new(console, flags)?.dispatch(),
            Self::Check(flags) => Check::new(console, flags)?.dispatch(),
            Self::Version => Version::new(console).dispatch(),
        }
    }
}
