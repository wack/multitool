pub use config::Cli;
pub use fs::manifest;
pub use terminal::Terminal;

pub use subsystems::{
    ActionListenerSubsystem, IngressSubsystem, MonitorSubsystem, PlatformSubsystem,
};

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
/// Contains the concrete metrics we can capture and observe.
mod metrics;
/// Our statistics library.
mod stats;
/// [subsystems] are structs that run as actors in the system, communicating
/// with each other through channels. They include Monitors, which read observations
/// from the system under management, ingresses, which control routing traffic to
/// user services, and platforms, which control the deployment of user services.
mod subsystems;
/// This module mediates communication with the terminal. This
/// lets us enforce our brand guidelines, respect user preferences for
/// color codes, and emojis, and ensure input from the terminal is consistent.
mod terminal;
mod utils;
