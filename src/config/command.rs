use clap::Subcommand;
use miette::Result;

use crate::cmd::Version;
use crate::terminal::Terminal;

/// A `MultiCommand` is one of the top-level commands accepted by
/// the multi CLI.
#[derive(Subcommand, Clone)]
pub enum MultiCommand {
    /// Print the CLI version and exit
    Version,
}

impl MultiCommand {
    /// dispatch the user-provided arguments to the command handler.
    pub async fn dispatch(&self, console: Terminal) -> Result<()> {
        match self.clone() {
            Self::Version => Version::new(console).dispatch(),
        }
    }
}
