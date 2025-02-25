use std::sync::Arc;

use async_trait::async_trait;
use mail::{DeployParams, PlatformMail, PromoteParams, YankParams};
use miette::{IntoDiagnostic, Report, Result};
use tokio::{
    select,
    sync::mpsc::{self, Receiver, Sender, channel},
    task::JoinHandle,
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::adapters::BoxedPlatform;

pub const PLATFORM_SUBSYSTEM_NAME: &str = "platform";
/// if you're going to pick an arbirary number, you could do worse
/// than picking a power of two.
const PLATFORM_MAILBOX_SIZE: usize = 1 << 4;

use mail::PlatformHandle;

mod mail;

/// The PlatformSubsystem handles sychronizing access to the
/// `[BoxedPlatform]` using message-passing and channels.
pub struct PlatformSubsystem {
    handle: PlatformHandle,
    platform: BoxedPlatform,
    mailbox: Receiver<PlatformMail>,
    shutdown: Receiver<()>,
}

impl PlatformSubsystem {
    pub fn new(platform: BoxedPlatform) -> Self {
        //   • We need to give the outside world a way to shutdown
        //     via the handle, and we don't yet have access to the
        //     subsystem. We could also do this by blocking the `handle()`
        //     method until we have the subsystem available, but that's trickier
        //     and potentially more deadlock-prone.
        let (shutdown_trigger, shutdown_signal) = channel(1);
        let (mail_outbox, mailbox) = mpsc::channel(PLATFORM_MAILBOX_SIZE);
        let shutdown = Arc::new(shutdown_trigger);
        let handle = PlatformHandle::new(Arc::new(mail_outbox), shutdown);
        Self {
            handle,
            platform,
            mailbox,
            shutdown: shutdown_signal,
        }
    }

    pub fn handle(&self) -> BoxedPlatform {
        Box::new(self.handle.clone())
    }

    async fn handle_deploy(&mut self, params: DeployParams) {
        let outbox = params.outbox;
        let result = self.platform.deploy().await;
        outbox.send(result).unwrap();
    }

    async fn handle_yank(&mut self, params: YankParams) {
        let outbox = params.outbox;
        let result = self.platform.yank_canary().await;
        outbox.send(result).unwrap();
    }

    async fn handle_promote(&mut self, params: PromoteParams) {
        let outbox = params.outbox;
        let result = self.platform.yank_canary().await;
        outbox.send(result).unwrap();
    }
}

#[async_trait]
impl IntoSubsystem<Report> for PlatformSubsystem {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        // Process all messages in a loop, while listening for shutdown.
        loop {
            select! {
                // Shutdown comes first so it has high priority.
                _ = subsys.on_shutdown_requested() => {
                    return self.platform.shutdown().await;
                }
                _ = self.shutdown.recv() => {
                    subsys.request_shutdown();
                }
                mail = self.mailbox.recv() => {
                    if let Some(mail) = mail {
                        match mail {
                            PlatformMail::DeployCanary(params) => self.handle_deploy(params).await,
                            PlatformMail::YankCanary(params) => self.handle_yank(params).await,
                            PlatformMail::PromoteDeployment(params) => self.handle_promote(params).await,
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::adapters::MockPlatform;
    use crate::{adapters::Platform, subsystems::platform::mail::PlatformHandle};

    use super::{PLATFORM_SUBSYSTEM_NAME, PlatformSubsystem};
    use miette::Report;
    use miette::Result;
    use static_assertions::assert_impl_all;
    use tokio::join;
    use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, Toplevel};

    assert_impl_all!(PlatformSubsystem: IntoSubsystem<Report>);
    assert_impl_all!(PlatformHandle: Platform, Clone);

    /// This test demonstrates how to use the PlatformSubsystem.
    /// It shows how you can launch it and call it's handle to
    /// perform actions, and shut it down manually.
    #[tokio::test]
    async fn use_platform_subsystem() -> Result<()> {
        // • We construct a mock platform to provide to the subsystem.
        let mut mock_platform = MockPlatform::new();
        mock_platform.expect_yank_canary().returning(|| Ok(()));
        let platform_subsys = PlatformSubsystem::new(Box::new(mock_platform));
        // • We create a handle so we can shutdown the system later.
        let mut handle = platform_subsys.handle();
        // • Launch the system.
        let system_fut = Toplevel::new(|s| async move {
            s.start(SubsystemBuilder::new(
                PLATFORM_SUBSYSTEM_NAME,
                platform_subsys.into_subsystem(),
            ));
        })
        .handle_shutdown_requests(Duration::from_millis(1000));
        let join_handle = tokio::spawn(system_fut);
        // • Yank the canary.
        assert!(handle.yank_canary().await.is_ok());
        // • Wait for shutdown.
        let (res1, res2) = join!(handle.shutdown(), join_handle);
        // • Check errors.
        assert!(res1.is_ok());
        assert!(res2.is_ok());
        Ok(())
    }
}
