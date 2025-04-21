#![deny(missing_docs)]

use std::path::PathBuf;

use clap::Args;
use derive_getters::Getters;

use crate::{
    MULTITOOL_ORIGIN,
    fs::{global_application, global_workspace},
};

#[derive(Args, Getters, Clone)]
pub struct RunSubcommand {
    #[arg(short, long, env = "MULTI_WORKSPACE", required = false, default_value = global_workspace())]
    workspace: String,
    #[arg(short, long, env = "MULTI_APPLICATION", required = true, default_value = global_application())]
    application: String,
    /// The path to the zipped serverless function.
    #[arg(value_name = "FILE")]
    artifact_path: PathBuf,

    #[arg(long, short = 'o', default_value = Some(MULTITOOL_ORIGIN))]
    origin: Option<String>,
}
