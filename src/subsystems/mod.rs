use async_trait::async_trait;
pub use controller::{CONTROLLER_SUBSYSTEM_NAME, ControllerSubsystem};
pub use ingress::{INGRESS_SUBSYSTEM_NAME, IngressSubsystem};
pub use monitor::{MONITOR_SUBSYSTEM_NAME, MonitorSubsystem};
pub use platform::{PLATFORM_SUBSYSTEM_NAME, PlatformSubsystem};
pub use relay::{RELAY_SUBSYSTEM_NAME, RelaySubsystem};

mod controller;
mod handle;
mod ingress;
mod monitor;
mod platform;
/// The relay subsystem is responsible for relaying messages
/// to and from the backend.
mod relay;

/// A ShutdownError is an error that occurred when a subsystem
/// was shutdown, or an error that forced the subsystem to shutdown.
pub type ShutdownResult = miette::Result<()>;

#[async_trait]
pub trait Shutdownable {
    async fn shutdown(&mut self) -> ShutdownResult;
}
