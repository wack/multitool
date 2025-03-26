use clap::Args;
use derive_getters::Getters;

#[derive(Args, Getters, Clone)]
pub struct LoginSubcommand {
    /// The email of the account
    #[clap(long)]
    email: Option<String>,
    /// The password of the account
    #[clap(long)]
    password: Option<String>,

    #[arg(long, short = 'o', default_value = Some("https://staging.api.multitool.run"))]
    origin: Option<String>,
}
