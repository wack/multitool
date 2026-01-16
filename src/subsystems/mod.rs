use async_trait::async_trait;
use miette::Diagnostic;

// Public API exports - allow unused since these are for external consumers
#[allow(unused_imports)]
pub use controller::{CONTROLLER_SUBSYSTEM_NAME, ControllerSubsystem};
// pub use error_logs::{ERROR_LOGS_SUBSYSTEM_NAME, ErrorLogsController};
#[allow(unused_imports)]
pub use ingress::{INGRESS_SUBSYSTEM_NAME, IngressHandle, IngressSubsystem};
#[allow(unused_imports)]
pub use monitor::{MONITOR_SUBSYSTEM_NAME, MonitorHandle, MonitorSubsystem};
#[allow(unused_imports)]
pub use platform::{PLATFORM_SUBSYSTEM_NAME, PlatformHandle, PlatformSubsystem};
#[allow(unused_imports)]
pub use relay::{RELAY_SUBSYSTEM_NAME, RelaySubsystem};

pub mod controller;
mod error_logs;
pub mod ingress;
pub mod monitor;
pub mod platform;
/// The relay subsystem is responsible for relaying messages
/// to and from the backend.
pub mod relay;

/// A ShutdownError is an error that occurred when a subsystem
/// was shutdown, or an error that forced the subsystem to shutdown.
pub type ShutdownResult = miette::Result<()>;

#[async_trait]
pub trait Shutdownable {
    async fn shutdown(&mut self) -> ShutdownResult;
}

/// This error occurs when an Optional value on a subsystem was taken
/// more than once. This is a usage error because those values are
/// only meant to be taken once, cloned if necessary.
#[derive(thiserror::Error, Debug, Diagnostic)]
#[error(
    "Internal error: the internal state of this type was corrupted by taking a value twice. Please report this error at https://github.com/wack/multitool/issues/new"
)]
pub(crate) struct TakenOptionalError;
