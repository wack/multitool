use clap::{Parser, command};
use derive_getters::Getters;
use tracing::level_filters::LevelFilter;

use super::colors::EnableColors;
use super::command::MultiCommand;

/// multi is a cloud deployment multitool.
#[derive(Getters, Parser)]
pub struct Cli {
    /// The subcommand to execute
    #[command(subcommand)]
    cmd: Option<MultiCommand>,

    /// Whether to color the output
    #[arg(long, global = true, value_enum, default_value_t=EnableColors::default())]
    enable_colors: EnableColors,

    /// Sets the maximum log level. Defaults to INFO. Options are
    /// 'trace', 'debug', 'info', 'warn', 'error', and 'off'. 'off' implies no logger will occur.
    /// Options are case-insensitive.
    #[arg(long, env, global = true, default_value_t = LevelFilter::INFO)]
    log_level: LevelFilter,
}
