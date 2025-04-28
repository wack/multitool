use clap::Args;
use derive_getters::Getters;

use crate::MULTITOOL_ORIGIN;

#[derive(Args, Getters, Clone)]
pub struct InitSubcommand {
    #[arg(long, short = 'o', default_value = Some(MULTITOOL_ORIGIN))]
    origin: Option<String>,
}
