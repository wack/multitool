use clap::Args;
use derive_getters::Getters;

use crate::MULTITOOL_ORIGIN;

#[derive(Args, Getters, Clone)]
pub struct LoginSubcommand {
    /// The email of the account
    #[clap(long)]
    email: Option<String>,
    /// The password of the account
    #[clap(long)]
    password: Option<String>,

    #[arg(long, short = 'o', default_value = Some(MULTITOOL_ORIGIN))]
    origin: Option<String>,
}
