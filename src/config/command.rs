use clap::Subcommand;
use miette::Result;

use crate::cmd::{Version};
use crate::terminal::Terminal;

/// A `WackCommand` is one of the top-level commands accepted by
/// the Wack CLI.
#[derive(Subcommand, Clone)]
pub enum WackCommand {
    /// Print the CLI version and exit
    Version,
}

impl WackCommand {
    /// dispatch the user-provided arguments to the command handler.
    pub async fn dispatch(&self, console: Terminal) -> Result<()> {
        match self.clone() {
            Self::Version => Version::new(console).dispatch(),
        }
    }
}
