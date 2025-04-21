use clap::{CommandFactory, Parser};
use miette::Result;

use multitool::{Cli, Terminal};

fn main() -> Result<()> {
    println!("Top of main.");
    // Parse the args provided to this process, including
    // commands and flags.
    println!("About to parse CLI.");
    let cli = Cli::parse();
    println!("CLI parse successful.");
    // Execute whichever command was requested.
    println!("About to dispatch command");
    dispatch_command(cli)
}

/// This function inspects the command that was provided and
/// delegates to its entrypoint.
fn dispatch_command(cli: Cli) -> Result<()> {
    println!("About to create new terminal");
    let terminal = Terminal::new(&cli);
    // Tell Miette whether it should write graphical errors
    // or use a more accessible output.
    println!("About to set error hook.");
    terminal.set_error_hook()?;
    println!("Error hook set successfully.");
    match cli.cmd() {
        Some(cmd) => cmd.clone().dispatch(terminal),
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
