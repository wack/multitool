use std::ops::ControlFlow;

use bon::bon;
use kameo::actor::{ActorId, ActorRef, Spawn, WeakActorRef};
use kameo::error::{ActorStopReason, Infallible};
use kameo::mailbox;
use kameo::Actor;
use tracing::{debug, trace};

use crate::adapters::{BackendClient, BoxedIngress, BoxedMonitor, BoxedPlatform, RolloutMetadata, StatusCode};
use crate::subsystems::ingress::IngressSubsystem;
use crate::subsystems::platform::PlatformSubsystem;
use crate::subsystems::relay::{RelaySubsystem, RelaySubsystemArgs};

use monitor::MonitorController;

/// This is the name as reported for logging.
pub const CONTROLLER_SUBSYSTEM_NAME: &str = "controller";

/// Arguments for ControllerSubsystem initialization.
pub struct ControllerSubsystemArgs {
    pub backend: BackendClient,
    pub monitor: BoxedMonitor,
    pub ingress: BoxedIngress,
    pub platform: BoxedPlatform,
    pub meta: RolloutMetadata,
}

impl ControllerSubsystemArgs {
    /// Spawn the controller and return the actor reference.
    pub fn spawn(self) -> ActorRef<ControllerSubsystem> {
        ControllerSubsystem::spawn_with_mailbox(self, mailbox::unbounded())
    }
}

/// The [ControllerSubsystem] is responsible for orchestrating all child subsystems.
/// It spawns and supervises the Ingress, Platform, MonitorController, and Relay subsystems.
pub struct ControllerSubsystem {
    /// References to child actors (populated on_start)
    #[allow(dead_code)]
    ingress_ref: Option<ActorRef<IngressSubsystem>>,
    #[allow(dead_code)]
    platform_ref: Option<ActorRef<PlatformSubsystem>>,
    #[allow(dead_code)]
    monitor_controller_ref: Option<ActorRef<MonitorController>>,
    #[allow(dead_code)]
    relay_ref: Option<ActorRef<RelaySubsystem<StatusCode>>>,
}

impl Actor for ControllerSubsystem {
    type Args = ControllerSubsystemArgs;
    type Error = Infallible;

    fn name() -> &'static str {
        CONTROLLER_SUBSYSTEM_NAME
    }

    async fn on_start(
        args: Self::Args,
        actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        debug!("ControllerSubsystem starting...");

        // Spawn IngressSubsystem
        let ingress_actor = IngressSubsystem::new(args.ingress);
        let ingress_ref = IngressSubsystem::spawn_with_mailbox(ingress_actor, mailbox::unbounded());
        actor_ref.link(&ingress_ref).await;
        let ingress_handle: BoxedIngress =
            Box::new(crate::subsystems::ingress::IngressHandle::new(ingress_ref.clone()));

        // Spawn PlatformSubsystem
        let platform_actor = PlatformSubsystem::new(args.platform);
        let platform_ref = PlatformSubsystem::spawn_with_mailbox(platform_actor, mailbox::unbounded());
        actor_ref.link(&platform_ref).await;
        let platform_handle: BoxedPlatform =
            Box::new(crate::subsystems::platform::PlatformHandle::new(platform_ref.clone()));

        // Spawn MonitorController
        let mut monitor_controller = MonitorController::builder().monitor(args.monitor).build();
        let observation_stream = monitor_controller.stream().expect("observation stream should be available");
        let baseline_sender = monitor_controller.get_baseline_sender();
        let canary_sender = monitor_controller.get_canary_sender();
        let monitor_controller_ref = monitor_controller.spawn();
        actor_ref.link(&monitor_controller_ref).await;

        // Spawn RelaySubsystem
        let relay_args = RelaySubsystemArgs {
            backend: args.backend,
            observations: observation_stream,
            platform: platform_handle,
            ingress: ingress_handle,
            meta: args.meta,
            backend_poll_frequency: None,
            baseline_sender,
            canary_sender,
        };
        let relay_ref = RelaySubsystem::spawn_with_args(relay_args);
        actor_ref.link(&relay_ref).await;

        debug!("ControllerSubsystem started all child actors");
        Ok(Self {
            ingress_ref: Some(ingress_ref),
            platform_ref: Some(platform_ref),
            monitor_controller_ref: Some(monitor_controller_ref),
            relay_ref: Some(relay_ref),
        })
    }

    async fn on_stop(
        &mut self,
        _actor_ref: WeakActorRef<Self>,
        reason: ActorStopReason,
    ) -> std::result::Result<(), Self::Error> {
        debug!("ControllerSubsystem stopped: {:?}", reason);
        Ok(())
    }

    async fn on_link_died(
        &mut self,
        actor_ref: WeakActorRef<Self>,
        id: ActorId,
        reason: ActorStopReason,
    ) -> std::result::Result<ControlFlow<ActorStopReason>, Self::Error> {
        debug!("Child actor {} died: {:?}", id, reason);
        // If any child dies, stop the controller (and thus all linked children)
        if let Some(strong_ref) = actor_ref.upgrade() {
            let _ = strong_ref.stop_gracefully().await;
        }
        Ok(ControlFlow::Break(ActorStopReason::LinkDied {
            id,
            reason: Box::new(reason),
        }))
    }
}

#[bon]
impl ControllerSubsystem {
    #[builder]
    pub fn new(
        backend: BackendClient,
        monitor: BoxedMonitor,
        ingress: BoxedIngress,
        platform: BoxedPlatform,
        meta: RolloutMetadata,
    ) -> ControllerSubsystemArgs {
        trace!("Creating a new controller subsystem...");

        ControllerSubsystemArgs {
            backend,
            monitor,
            ingress,
            platform,
            meta,
        }
    }

    /// Spawn the controller and return the actor reference.
    pub fn spawn_controller(args: ControllerSubsystemArgs) -> ActorRef<Self> {
        Self::spawn_with_mailbox(args, mailbox::unbounded())
    }
}

/// Contains the controller for the monitor.
pub mod monitor;

#[cfg(test)]
mod tests {
    use super::ControllerSubsystem;
    use kameo::Actor;
    use static_assertions::assert_impl_all;

    assert_impl_all!(ControllerSubsystem: Actor);
}
