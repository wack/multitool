use clap::{CommandFactory, Parser};
use miette::Result;

use multitool::{Cli, Terminal};

fn main() -> Result<()> {
    // Parse the args provided to this process, including
    // commands and flags.
    let cli = Cli::parse();
    // Execute whichever command was requested.
    dispatch_command(cli)
}

/// This function inspects the command that was provided and
/// delegates to its entrypoint.
fn dispatch_command(cli: Cli) -> Result<()> {
    let terminal = Terminal::new(&cli);
    // Tell Miette whether it should write graphical errors
    // or use a more accessible output.
    terminal.set_error_hook()?;
    match cli.cmd() {
        Some(cmd) => cmd.clone().dispatch(terminal, &cli),
        // No command was provided.
        None => empty_command(),
    }
}

/// When the CLI is run without any commands, we print
/// the help text and exit successfully.
fn empty_command() -> Result<()> {
    Cli::command()
        .print_long_help()
        .expect("unable to print help message");
    Ok(())
}
