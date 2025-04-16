use miette::Result;
use miette::WrapErr;

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
        let mut err: Result<()> = Err(InnerError.into());
        for i in 0..8 {
            let msg = format!("Error level {i}");
            err = err.wrap_err(msg);
        }
        return err;
        // let second_error = inner.wrap_err("Wrapper message.");
        // let third_error = second_error.wrap_err("Third layer");
        // let fourth
        // return third_error;
        // self.terminal.print_version(CLI_VERSION)
    }
}

use miette::Diagnostic;
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
#[error("Inner error")]
struct InnerError;
