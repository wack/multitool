use async_trait::async_trait;
use miette::{IntoDiagnostic, Report, Result};
use tokio::{sync::{mpsc::{self, Receiver, Sender}, oneshot}, task::JoinHandle};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};

use crate::adapters::BoxedIngress;

pub const INGRESS_SUBSYSTEM_NAME: &str = "ingress";
/// If you're going to pick an arbitrary number, you could do worse
/// than picking a power of two.
const INGRESS_MAILBOX_SIZE: usize = 1<<4;

/// The IngressSubsystem handles synchronizing access to the 
/// `BoxedIngress` using message-passing and channels.
pub struct IngressSubsystem {
    /// This is where we write messages for the `[BoxedIngress]` to receive.
    outbox: Sender<IngressMail>,
    thread_done: JoinHandle<()>,
}

/// We anticipate changing this number in the future, so for now we're
/// just going to use a type alias to keep everything localized to one spot.
/// In the prototype, we used a "WholeNumber" percentage to ensure the
/// user only put in a number from 0-100. We can imagine using fractions,
/// but we want to validate that the number is between [0..100] regardless
/// of whether we use fractions or not.
type CanaryTrafficPercent = u32;

impl IngressSubsystem {
    pub fn new(mut ingress: BoxedIngress) -> Self {
        // • Spawn a new task with the BoxedIngress and the mailbox.
        let (outbox, mut inbox) = mpsc::channel(INGRESS_MAILBOX_SIZE);
        // • Spawn the thread that reads from the mailbox and processes
        //   each request.
        let join_handler = tokio::spawn(async move {
            while let Some(mail) = inbox.recv().await {
                match mail {
                    IngressMail::SetCanaryTraffic(traffic_params) => {
                        let outbox = traffic_params.outbox;
                        let traffic = traffic_params.percent;
                        let result = ingress.set_canary_traffic(traffic).await;
                        outbox.send(result).unwrap();
                    }
                }
            }
            println!("Ingress shutting down.");
        });
        // Return the Subsystem, storing the outbox and the join handle.
        Self {
            thread_done: join_handler,
            outbox,
        }
    }
}

impl IngressSubsystem {
    pub async fn set_canary_traffic(&self, percent: CanaryTrafficPercent) -> Result<()> {
        let (sender, receiver): (oneshot::Sender<Result<()>>, oneshot::Receiver<Result<()>>) = oneshot::channel();
        let params = TrafficParams::new(sender, percent);
        let mail = IngressMail::SetCanaryTraffic(params);
        self.outbox.send(mail).await.into_diagnostic()?;
        receiver.await.into_diagnostic()?
    }
}

enum IngressMail {
    SetCanaryTraffic(TrafficParams),
}

impl IngressMail {
    pub async fn set_canary_traffic(percent: CanaryTrafficPercent) -> TrafficResp {
        todo!();
    }
}

struct TrafficParams {
    /// The sender where the response is written.
    outbox: oneshot::Sender<TrafficResp>,
    /// The amount of traffic the user is expected to receive.
    percent: u32,
}

impl TrafficParams {
    fn new(outbox: oneshot::Sender<TrafficResp>, percent: CanaryTrafficPercent) -> Self {
        Self { outbox, percent }
    }
}

type TrafficResp = Result<()>;

#[async_trait]
impl IntoSubsystem<Report> for IngressSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::IngressSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(IngressSubsystem: IntoSubsystem<Report>);
}
