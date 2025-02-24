use std::sync::Arc;

use async_trait::async_trait;
use miette::{Report, Result};
use tokio::{pin, time::interval};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemHandle};
use tokio_stream::{StreamExt, wrappers::IntervalStream};

use crate::adapters::{BackendClient, Monitor};
use crate::stats::Observation;

/// The maximum number of observations that can be recevied before we
/// recompute statistical significance.
/// If this number is too low, we'll be performing compute-intensive
/// statical tests very often. If this number is too high, we could
/// be waiting too long before computing, which could permit us to promote more eagerly.
const DEFAULT_BATCH_SIZE: usize = 512;

/// The [ControllerSubsystem] is responsible for talking to the backend.
/// It sends new monitoring observations, asks for instructions to perform
/// on cloud resources, and reports the state of those instructions back
/// to the backend.
pub struct ControllerSubsystem {
    backend: Arc<dyn BackendClient + 'static>,
}

impl ControllerSubsystem {
    pub fn new(backend: Arc<dyn BackendClient>) -> Self {
        Self { backend }
    }
}

/// This is the name as reported to the `TopLevelSubsystem`,
/// presumably for logging.
pub const CONTROLLER_SUBSYSTEM_NAME: &str = "controller";

#[async_trait]
impl IntoSubsystem<Report> for ControllerSubsystem {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        // Spawn a thread that calls the monitor on a timer.
        //   * Convert the results into a stream.
        //   * Consume the stream in a thread and push the results
        //     to the backend.
        // Poll the backend for new states to effect.
        //   * Spawn a thread that runs on a timer.
        todo!()
    }
}

/*
// TODO: Add a call to chunk_timeout to ensure that items are arriving after a particular
//       amount of time.
/// [repeat_query] runs the query on an interval and returns a stream of items.
/// This function runs indefinitely.
pub fn repeat_query<T: Observation>(
    mut observer: Box<dyn Monitor<Item = T>>,
    duration: tokio::time::Duration,
) -> impl tokio_stream::Stream<Item = T> {
    // • Everything happens in this stream closure, which desugars
    //   into a background thread and a channel write at yield points.
    async_stream::stream! {
        // • Initialize a timer that fires every interval.
        let timer = IntervalStream::new(interval(duration));
        // • The timer must be pinned to use in an iterator
        //   because we must promise that its address must not
        //   be moved between iterations.
        pin!(timer);
        // Each iteration of the loop represents one unit of tiem.
        while timer.next().await.is_some() {
            // • We perform the query then dump the results into the stream.
            let items = observer.query().await;
            for item in items {
                yield item;
            }
        }
    }
}

// TODO: Honestly, this function can be inlined where used.
/// Batch observations together into maximally sized chunks, and dump
/// them to a stream every so often.
pub fn batch_observations<T: Observation>(
    obs: impl tokio_stream::Stream<Item = T>,
    duration: tokio::time::Duration,
) -> impl tokio_stream::Stream<Item = Vec<T>> {
    obs.chunks_timeout(DEFAULT_BATCH_SIZE, duration)
}
*/

#[cfg(test)]
mod tests {
    use super::ControllerSubsystem;
    use miette::Report;
    use static_assertions::assert_impl_all;
    use tokio_graceful_shutdown::IntoSubsystem;

    assert_impl_all!(ControllerSubsystem: IntoSubsystem<Report>);
}
