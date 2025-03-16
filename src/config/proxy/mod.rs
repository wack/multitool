use clap::Args;
use derive_getters::Getters;

#[derive(Args, Clone, Getters)]
pub struct ProxySubcommand {
    #[clap(long, short = 'b')]
    baseline: String,
    #[clap(long, short = 'c')]
    canary: String,
}
