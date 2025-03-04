use async_trait::async_trait;
use chrono::{DateTime, Utc};
use derive_getters::Getters;
use miette::{Report, Result};
use multitool_sdk::models::DeploymentState;
use tokio::{
    select,
    sync::{
        mpsc::{self, Receiver, Sender},
        oneshot,
    },
    time::interval,
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle};

use crate::{
    Shutdownable,
    adapters::{BackendClient, DeploymentMetadata, LockedState},
    subsystems::{ShutdownResult, relay::renewer::LeaseRenewer},
};

/// If you're going to pick an arbitrary number, you could do
/// worse than picking a power of 2.
const DEFAULT_STREAM_SIZE: usize = 1 << 5;

pub(super) struct LeaseManagementSubsystem {
    backend: BackendClient,
    /// Incoming requests for leases.
    inbox: Receiver<DeploymentState>,
    /// Stream out the list of leased items.
    outbox: Sender<LockedState>,
    out_stream: Option<Receiver<LockedState>>,
    // Hold onto the deployment ID, application id, and workspace ID.
    meta: DeploymentMetadata,
}

impl LeaseManagementSubsystem {
    pub fn new(
        backend: BackendClient,
        inbox: Receiver<DeploymentState>,
        meta: DeploymentMetadata,
    ) -> Self {
        let (outbox, out_stream) = mpsc::channel(DEFAULT_STREAM_SIZE);
        Self {
            backend,
            inbox,
            outbox,
            out_stream: Some(out_stream),
            meta,
        }
    }

    pub fn take_stream(&mut self) -> Option<Receiver<LockedState>> {
        self.out_stream.take()
    }
}

// TODO: Well, it appears we don't actually
//       need a separate LeaseManager from the
//       renewal manager. We can just spawn
//       a lease manager whenever we need it.
//       We should delete this file and roll it up into
//       the RelaySubsystem, but we want to use the body
//       of the LeaseRenewer file as the new LeaseManager,
//       (renaming it).

#[async_trait]
impl IntoSubsystem<Report> for LeaseManagementSubsystem {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        loop {
            select! {
                // Wait for shutdown
                _ = subsys.on_shutdown_requested() => {
                    return self.shutdown().await;
                }
                // Listen for new lease requests.
                req = self.inbox.recv() => {
                    if let Some(req) =  req {
                        // When one comes in, lease the state, then
                        // spawn a task to renew the lease until its completed.
                        if let Ok(lock) = self.backend.lock_state(&self.meta, req).await {
                            // Spawn a thread that will periodically renew the lease.
                            // ...and listen for the shutdown signal.
                            // Attempt to renew the lease every so often, with half
                            // the time left in the lease so we don't cut it close.
                            let period = *lock.period()/2;
                            let mut lease_renewer = LeaseRenewer::new(self.meta.clone(),
                                req,
                                self.backend.clone(),
                                period,
                            );
                            let renewer_handle = lease_renewer.take().unwrap();
                            subsys.start(SubsystemBuilder::new("foobar", lease_renewer.into_subsystem()));
                            self.outbox.send(LockedState{
                                task_done: renewer_handle,
                            }).await.unwrap();
                        }
                    } else {
                        // The stream has been closed, so we should shutdown.
                        return self.shutdown().await;
                    }
                }

            }
        }
    }
}

#[async_trait]
impl Shutdownable for LeaseManagementSubsystem {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!("TODO: Release all locks (if possible?)");
    }
}
