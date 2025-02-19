use std::sync::Arc;

use async_trait::async_trait;
use mail::PlatformMail;
use miette::{IntoDiagnostic, Report, Result};
use tokio::{
    select,
    sync::mpsc::{self, channel},
    task::JoinHandle,
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::{adapters::BoxedPlatform, artifacts::LambdaZip};

pub const PLATFORM_SUBSYSTEM_NAME: &str = "platform";
/// if you're going to pick an arbirary number, you could do worse
/// than picking a power of two.
const PLATFORM_MAILBOX_SIZE: usize = 1 << 4;

use super::{ShutdownResult, Shutdownable};
use mail::PlatformHandle;

mod mail;

/// The PlatformSubsystem handles sychronizing access to the
/// `[BoxedPlatform]` using message-passing and channels.
pub struct PlatformSubsystem {
    task_done: JoinHandle<ShutdownResult>,
    handle: PlatformHandle,
}

impl PlatformSubsystem {
    pub fn new(artifact: LambdaZip, mut platform: BoxedPlatform) -> Self {
        // • Spawn a new task with the BoxedPlatform and the mailbox.
        let (shutdown_trigger, mut shutdown_signal) = channel(1);
        let (mail_outbox, mut mail_inbox) = mpsc::channel(PLATFORM_MAILBOX_SIZE);
        let task_done = tokio::spawn(async move {
            loop {
                select! {
                    _ = shutdown_signal.recv() => {
                        return platform.shutdown().await;
                    }
                    mail = mail_inbox.recv() => {
                        if let Some(mail) = mail {
                            match mail {
                                PlatformMail::DeployCanary(deploy_params) => {
                                    let outbox = deploy_params.outbox;
                                    let result = platform.deploy().await;
                                    outbox.send(result).unwrap();
                                }
                                PlatformMail::YankCanary(rollback_params) => {
                                    let outbox = rollback_params.outbox;
                                    let result = platform.yank_canary().await;
                                    outbox.send(result).unwrap();
                                }
                                PlatformMail::PromoteDeployment(promote_params) => {
                                    let outbox = promote_params.outbox;
                                    let result = platform.yank_canary().await;
                                    outbox.send(result).unwrap();
                                }
                            }
                        }
                    }
                }
            }
        });
        let shutdown = Arc::new(shutdown_trigger);
        let handle = PlatformHandle::new(Arc::new(mail_outbox), shutdown);
        Self { task_done, handle }
    }

    pub fn handle(&self) -> PlatformHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl IntoSubsystem<Report> for PlatformSubsystem {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        subsys.on_shutdown_requested().await;
        // Propagate the shutdown signal.
        let shutdown_result = self.handle.shutdown().await;
        // Wait for the thread to shutdown.
        let task_result = self.task_done.await.into_diagnostic();
        shutdown_result.and(task_result?)
    }
}

#[cfg(test)]
mod tests {
    use crate::{adapters::Platform, subsystems::platform::mail::PlatformHandle};

    use super::PlatformSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(PlatformSubsystem: IntoSubsystem<Report>);
    assert_impl_all!(PlatformHandle: Platform, Clone);
}
