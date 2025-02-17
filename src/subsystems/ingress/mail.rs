use miette::Result;
use tokio::sync::oneshot;

use super::CanaryTrafficPercent;

pub(super) enum IngressMail {
    SetCanaryTraffic(TrafficParams),
}

pub(super) struct TrafficParams {
    /// The sender where the response is written.
    pub(super) outbox: oneshot::Sender<TrafficResp>,
    /// The amount of traffic the user is expected to receive.
    pub(super) percent: u32,
}

impl TrafficParams {
    pub(super) fn new(outbox: oneshot::Sender<TrafficResp>, percent: CanaryTrafficPercent) -> Self {
        Self { outbox, percent }
    }
}

pub(super) type TrafficResp = Result<()>;
