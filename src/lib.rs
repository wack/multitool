pub use config::Flags;
pub use fs::manifest;
pub use terminal::Terminal;

/// `adapters` are traits that describe some external, effective
/// service, like an AWS Lambda or a message bus, for example.
mod adapters;
/// For loading and handling various artifacts.
/// Currently, we expect all artifacts to be  zipped
/// lambda functions.
pub mod artifacts;
/// Contains the dispatch logic for running individual CLI subcommands.
/// The CLI's main function calls into these entrypoints for each subcommand.
mod cmd;
/// configuration of the CLI, either from the environment of flags.
mod config;
/// An abstraction over the user's filesystem, respecting $XFG_CONFIG.
mod fs;
/// This module mediates communication with the terminal. This
/// lets us enforce our brand guidelines, respect user preferences for
/// color codes, and emojis, and ensure input from the terminal is consistent.
mod terminal;
