pub use cli::Cli;
pub use init::InitSubcommand;
pub use login::LoginSubcommand;
pub use run::RunSubcommand;

#[cfg(feature = "proxy")]
pub use proxy::ProxySubcommand;

#[cfg(feature = "gateway")]
pub use gateway::{GatewayMode, GatewaySubcommand};

mod cli;
mod colors;
mod command;
mod gateway;
mod init;
mod login;
mod proxy;
mod run;
