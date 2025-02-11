use std::path::PathBuf;

use clap::Args;

#[derive(Args, Clone)]
pub struct RunSubcommand {
    #[arg(short, long, env = "MULTI_WORKSPACE")]
    pub workspace: String,
    #[arg(short, long, env = "MULTI_APPLICATION")]
    pub application: String,
    /// The path to the zipped serverless function.
    #[arg(value_name = "FILE")]
    pub artifact_path: PathBuf,
}
