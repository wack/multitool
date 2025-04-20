use clap::{CommandFactory, Parser};
use multitool::{Cli, Terminal};

fn main() {
    // Parse the args provided to this process, including
    // commands and flags.
    let markdown: String = clap_markdown::help_markdown::<Cli>();
    println!("{markdown}");
}
