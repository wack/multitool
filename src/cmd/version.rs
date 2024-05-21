use miette::Result;

use crate::Terminal;

/// This is the version of the multi CLI, pulled from Cargo.toml.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Print the CLI version to stdout.
pub struct Version {
    terminal: Terminal,
}

impl Version {
    pub fn new(terminal: Terminal) -> Self {
        Self { terminal }
    }

    /// Print the version and exit.
    pub fn dispatch(self) -> Result<()> {
        self.terminal.print_version(CLI_VERSION)
    }
}
