use std::path::{Path, PathBuf};

use clap::Args;

/// `multi check`: validate the requirements declared in `CHECKS.md` files.
///
/// Model/provider flags are intentionally absent — configuration is hardcoded
/// for the MVP (see M2). Only the working directory is configurable.
#[derive(Args, Clone)]
pub struct CheckSubcommand {
    /// The directory to recursively scan for `CHECKS.md` files.
    #[arg(default_value = ".")]
    directory: PathBuf,
}

impl CheckSubcommand {
    /// The directory to scan (defaults to the current directory).
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}
