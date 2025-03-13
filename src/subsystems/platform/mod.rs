use std::sync::Arc;

use async_trait::async_trait;
use mail::{DeleteParams, DeployParams, PlatformMail, PromoteParams, YankParams};
use miette::{Report, Result};
use tokio::{
    select,
    sync::mpsc::{self, Receiver, channel},
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::adapters::BoxedPlatform;

pub const PLATFORM_SUBSYSTEM_NAME: &str = "platform";
/// if you're going to pick an arbirary number, you could do worse
/// than picking a power of two.
const PLATFORM_MAILBOX_SIZE: usize = 1 << 4;

use mail::PlatformHandle;

use super::{ShutdownResult, Shutdownable};

mod mail;

/// The PlatformSubsystem handles sychronizing access to the
/// `[BoxedPlatform]` using message-passing and channels.
pub struct PlatformSubsystem {
    platform: BoxedPlatform,
    handle: PlatformHandle,
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
        let (shutdown_trigger, shutdown_recv) = channel(1);
        let (mail_outbox, mailbox) = mpsc::channel(PLATFORM_MAILBOX_SIZE);
        let shutdown_sender = Arc::new(shutdown_trigger);
        let handle = PlatformHandle::new(Arc::new(mail_outbox), shutdown_sender);
        Self {
            handle,
            platform,
            mailbox,
            shutdown: shutdown_recv,
        }
    }

    pub fn handle(&self) -> BoxedPlatform {
        Box::new(self.handle.clone())
    }

    async fn respond_to_mail(&mut self, mail: PlatformMail) {
        match mail {
            PlatformMail::DeployCanary(params) => self.handle_deploy(params).await,
            PlatformMail::YankCanary(params) => self.handle_yank(params).await,
            PlatformMail::DeleteCanary(params) => self.handle_delete(params).await,
            PlatformMail::PromoteDeployment(params) => self.handle_promote(params).await,
        }
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

    async fn handle_delete(&mut self, params: DeleteParams) {
        let outbox = params.outbox;
        let result = self.platform.delete_canary().await;
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
                    return self.shutdown().await;
                }
                // Shutdown signal from one of the handles. Since this thread has exclusive
                // access to the platform, we have to give the outside world a way to shut
                // us down. That's this channel, created before the SubsystemHandle existed.
                _ = self.shutdown.recv() => {
                    return self.shutdown().await;
                }
                mail = self.mailbox.recv() => {
                    if let Some(mail) = mail {
                        self.respond_to_mail(mail).await;
                    } else {
                        subsys.request_shutdown()
                    }
                }
            }
        }
    }
}

#[async_trait]
impl Shutdownable for PlatformSubsystem {
    async fn shutdown(&mut self) -> ShutdownResult {
        // We just have to shut the platform down manually,
        // since we have an exclusive lock on it.
        self.platform.shutdown().await
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
