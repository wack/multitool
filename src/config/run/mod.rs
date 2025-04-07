use std::path::PathBuf;

use clap::Args;
use derive_getters::Getters;

use crate::MULTITOOL_ORIGIN;

#[derive(Args, Getters, Clone)]
pub struct RunSubcommand {
    #[arg(short, long, env = "MULTI_WORKSPACE")]
    workspace: String,
    #[arg(short, long, env = "MULTI_APPLICATION")]
    application: String,
    /// The path to the zipped serverless function.
    #[arg(value_name = "FILE")]
    artifact_path: PathBuf,

    #[arg(long, short = 'o', default_value = Some(MULTITOOL_ORIGIN))]
    origin: Option<String>,
}
