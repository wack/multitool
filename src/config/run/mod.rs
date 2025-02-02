use std::path::PathBuf;

use clap::Args;

#[derive(Args, Clone)]
pub struct RunSubcommand {
    /// The path to the zipped serverless function.
    #[arg(value_name = "FILE")]
    artifact_path: PathBuf,
}

impl RunSubcommand {
    /// Return the user's email, if provided via the CLI.
    pub fn artifact_path(&self) -> &PathBuf {
        &self.artifact_path
    }
}
