use async_trait::async_trait;
use miette::{Report, Result, miette};
use tokio::{
    pin,
    sync::oneshot,
    time::{Duration, interval},
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use tokio_stream::Stream;
use tokio_stream::{StreamExt as _, wrappers::IntervalStream};

use crate::{adapters::BoxedMonitor, stats::Observation};

pub(super) const MONITOR_CONTROLLER_SUBSYSTEM_NAME: &str = "controller/monitor";

/// The maximum number of observations that can be recevied before we
/// recompute statistical significance.
/// If this number is too low, we'll be performing compute-intensive
/// statical tests very often. If this number is too high, we could
/// be waiting too long before computing, which could permit us to promote more eagerly.
const DEFAULT_MAX_BATCH_SIZE: usize = 512;

/// Check for new metrics every 60s. TODO: This number is set because
/// AWS Cloudwatch only populates new metrics once per minute. This number
/// can be adjusted in the future.
const DEFAULT_OBSERVATION_PERIOD: Duration = Duration::from_secs(60);

pub struct MonitorController {
    shutdown: oneshot::Sender<()>,
}

impl MonitorController {
    pub fn launch<T: Observation>(mut monitor: BoxedMonitor<T>) -> (Self, impl Stream<Item = T>) {
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        // • Everything happens in this stream closure, which desugars
        //   into a background thread and a channel write at yield points.
        let stream = async_stream::stream! {
            // • Initialize a timer that fires every interval.
            let timer = IntervalStream::new(interval(DEFAULT_OBSERVATION_PERIOD));
            // • The timer must be pinned to use in an iterator
            //   because we must promise that its address must not
            //   be moved between iterations.
            pin!(timer);
            // TODO:
            //   Select in a loop instead of while-looping here.
            // TODO:
            //   Print the error message instead of unwrapping, below.
            // TODO: Remember to batch observations instead of returning them
            // immediately.
            // Each iteration of the loop represents one unit of tiem.
            while timer.next().await.is_some() {
                // • We perform the query then dump the results into the stream.
                let items = monitor.query().await.unwrap();
                for item in items {
                    yield item;
                }
            }
        };
        (
            Self {
                shutdown: shutdown_sender,
            },
            stream,
        )
    }
}

#[async_trait]
impl IntoSubsystem<Report> for MonitorController {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        subsys.on_shutdown_requested().await;
        self.shutdown
            .send(())
            .map_err(|_| miette!("Could not send shutdown signal"))
    }
}
