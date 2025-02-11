use clap::{command, Parser};

use super::colors::EnableColors;
use super::command::MultiCommand;

/// multi is a cloud deployment multitool.
#[derive(Parser)]
pub struct Flags {
    /// The subcommand to execute
    #[command(subcommand)]
    cmd: Option<MultiCommand>,

    /// Whether to color the output
    #[arg(long, value_enum, default_value_t=EnableColors::default())]
    enable_colors: EnableColors,

    #[arg(long, short = 'o', default_value = Some("localhost:8000"))]
    origin: Option<String>,
}

impl Flags {
    /// Return the top-level command provided, if it exists.
    pub fn cmd(&self) -> Option<&MultiCommand> {
        self.cmd.as_ref()
    }

    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// Getter that returns the user-provided preference
    /// for using color codes in the terminal output.
    pub fn enable_colors(&self) -> EnableColors {
        self.enable_colors
    }
}
