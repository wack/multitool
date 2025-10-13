use clap::Args;
use derive_getters::Getters;

use crate::MULTITOOL_ORIGIN;

#[derive(Args, Getters, Clone)]
pub struct LoginSubcommand {
    /// The email of the account
    #[arg(short, long, env = "MULTI_EMAIL")]
    email: Option<String>,
    /// The password of the account
    #[arg(short, long, env = "MULTI_PASSWORD")]
    password: Option<String>,

    #[arg(long, short = 'o', env = "MULTI_ORIGIN", default_value = Some(MULTITOOL_ORIGIN))]
    origin: Option<String>,
}
