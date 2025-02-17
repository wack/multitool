use std::sync::Arc;

use async_trait::async_trait;
use mail::PlatformMail;
use miette::{Report, Result};
use tokio::{
    sync::mpsc::{self},
    task::JoinHandle,
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::{adapters::BoxedPlatform, artifacts::LambdaZip};

pub const PLATFORM_SUBSYSTEM_NAME: &str = "platform";
/// if you're going to pick an arbirary number, you could do worse
/// than picking a power of two.
const PLATFORM_MAILBOX_SIZE: usize = 1 << 4;

pub use handle::PlatformHandle;

mod handle;
mod mail;

/// The PlatformSubsystem handles sychronizing access to the
/// `[BoxedPlatform]` using message-passing and channels.
pub struct PlatformSubsystem {
    artifact: LambdaZip,
    handle: PlatformHandle,
    task_done: JoinHandle<()>,
}

impl PlatformSubsystem {
    pub fn new(artifact: LambdaZip, mut platform: BoxedPlatform) -> Self {
        // • Spawn a new task with the BoxedPlatform and the mailbox.
        let (outbox, mut inbox) = mpsc::channel(PLATFORM_MAILBOX_SIZE);
        let join_handle = tokio::spawn(async move {
            while let Some(mail) = inbox.recv().await {
                match mail {
                    PlatformMail::DeployCanary(deploy_params) => {
                        let outbox = deploy_params.outbox;
                        let result = platform.deploy().await;
                        outbox.send(result).unwrap();
                    }
                    PlatformMail::RollbackCanary(rollback_params) => {
                        let outbox = rollback_params.outbox;
                        let result = platform.rollback_canary().await;
                        outbox.send(result).unwrap();
                    }
                    PlatformMail::PromoteCanary(promote_params) => {
                        let outbox = promote_params.outbox;
                        let result = platform.rollback_canary().await;
                        outbox.send(result).unwrap();
                    }
                }
            }
        });
        let obj_handle = PlatformHandle::new(Arc::new(outbox));
        Self {
            artifact,
            task_done: join_handle,
            handle: obj_handle,
        }
    }

    pub fn handle(&self) -> PlatformHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl IntoSubsystem<Report> for PlatformSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        subsys.on_shutdown_requested().await;
        // TODO: Do we need a second channel to shutdown with priority?
        // The channel will be shutdown when all copies of the sender
        // are dropped.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PlatformSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(PlatformSubsystem: IntoSubsystem<Report>);
}
