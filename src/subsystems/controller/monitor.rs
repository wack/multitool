use std::time::Duration;

use async_trait::async_trait;
use bon::bon;
use futures_util::TryStreamExt;
use miette::{Report, Result};
use tokio::{
    pin, select,
    sync::mpsc::{self, Receiver, Sender},
    time::interval,
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle};
use tokio_stream::{Stream, StreamExt as _, wrappers::IntervalStream};
use tracing::debug;

use crate::{
    MonitorSubsystem,
    adapters::{BoxedMonitor, StatusCode},
    stats::Observation,
    subsystems::{MONITOR_SUBSYSTEM_NAME, TakenOptionalError},
};

/// The maximum number of observations that can be recevied before we
/// emit the results to the backend.
/// If this number is too low, we'll be performing compute-intensive
/// statical tests very often and sending many requests over the wire.
/// If this number is too high, we could be waiting too long before
/// computing, which could permit us to promote more eagerly.
const DEFAULT_MAX_BATCH_SIZE: usize = 512;
/// The frequency with which we poll the `Monitor` for new results.
/// For AWS Cloudwatch, they update their autocollcted metrics every
/// minute. So polling every 30s cuts down on the time between
/// when the data is uploaded and when we receive it.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(60);
/// The frequency with which we emit data from the controller,
/// (usually to go to the backend).
const DEFAULT_EMIT_INTERVAL: Duration = Duration::from_secs(60);

pub const MONITOR_CONTROLLER_SUBSYSTEM_NAME: &str = "controller/monitor";

/// The `MonitorController` is responsible for scheduling calls
/// to the `Monitor` on a timer, and batching the results. This
/// decouples how often we *gather* metrics from how often to
/// *store* them.
///
/// Its a "controller" in the sense of PID controller, not in the sense
/// of Model-View-Controller.
pub struct MonitorController<T>
where
    T: Observation,
{
    monitor: BoxedMonitor,
    /// This field stores the stream of outputs.
    /// The stream can only be given to one caller. The first
    /// call to `Self::stream` will return the stream, and all
    /// subsequent calls will return None.
    recv: Option<Receiver<Vec<T>>>,
    sender: Sender<Vec<T>>,
    poll_interval: Duration,
    emit_interval: Duration,
    on_error: Box<dyn Fn(&miette::Report) + Send + Sync>,
}

#[bon]
impl MonitorController<StatusCode> {
    #[builder]
    pub fn new(
        monitor: BoxedMonitor,
        poll_interval: Option<Duration>,
        emit_interval: Option<Duration>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(DEFAULT_MAX_BATCH_SIZE);
        Self {
            monitor,
            sender,
            recv: Some(receiver),
            poll_interval: poll_interval.unwrap_or(DEFAULT_POLL_INTERVAL),
            emit_interval: emit_interval.unwrap_or(DEFAULT_EMIT_INTERVAL),
            on_error: Box::new(log_error),
        }
    }

    /// This function returns a channel receiver of values the first time
    /// its called. Subsequent calls return None.
    pub fn stream(&mut self) -> Result<Receiver<Vec<StatusCode>>> {
        self.recv.take().ok_or(TakenOptionalError.into())
        // TODO: This block of code produces an Unpin error at the caller
        //       when using a Stream instead of a receiver, but its
        //       a more idiomatic API. If we can work through the error,
        //       it would be better than returning a receiver.
        // self.recv.take().map(|mut receiver| {
        //     async_stream::stream! {
        //         while let Some(item) = receiver.recv().await {
        //             yield item;
        //         }
        //     }
        // })
    }

    #[allow(dead_code)]
    pub fn set_error_hook<F>(&mut self, func: F)
    where
        F: Fn(&miette::Report) + Send + Sync + 'static,
    {
        self.on_error = Box::new(func)
    }
}

#[async_trait]
impl IntoSubsystem<Report> for MonitorController<StatusCode> {
    async fn run(mut self, subsys: SubsystemHandle) -> Result<()> {
        // • Build the `MonitorSubsystem`. Don't launch it until
        //   we take a handle to it.
        let monitor_subsystem = MonitorSubsystem::new(self.monitor);
        // • Capture a handle to the subsystem so we can call `Monitor::query`.
        let handle = monitor_subsystem.handle();

        // • Launch the subsystem.
        subsys.start(SubsystemBuilder::new(
            MONITOR_SUBSYSTEM_NAME,
            monitor_subsystem.into_subsystem(),
        ));

        // Now, we can periodically poll the monitor for
        // new data.
        // • First, schedule the Monitor to be queried every so often.
        let query_stream = repeat_query(handle, self.poll_interval)
            .inspect_err(|e| (self.on_error)(e))
            .filter_map(Result::ok);
        // • Next, aggregate query results and emit them every so often.
        let chunked_stream =
            query_stream.chunks_timeout(DEFAULT_MAX_BATCH_SIZE, self.emit_interval);
        // Now, chunked_stream will emit batches of observations
        // every `emit_interval`. We can ship those results up
        // to the caller now.
        pin!(chunked_stream);
        loop {
            select! {
                _ = subsys.on_shutdown_requested() => {
                    // If we've received the shutdown signal,
                    // we don't have anything to do except ensure
                    // our children have shutdown, guaranteeing
                    // the monitor is shut down.
                    // NB: We can't implement the shutdown trait because
                    // self has been partially moved.
                    subsys.wait_for_children().await;
                    return Ok(());
                }
                next = chunked_stream.next() => {
                    if let Some(batch) = next {
                        // We received a new batch of observations.
                        // Let's emit them to our output stream.
                        self.sender.send(batch).await.unwrap();
                    } else {
                        debug!("Shutting down in monitor");
                        // The stream has been closed. Shut down.
                        subsys.request_local_shutdown();
                    }
                }
            }
        }
    }
}

fn log_error(err: &Report) {
    tracing::error!("Error while collecting monitoring data: {err}");
}

/// [repeat_query] runs the query on an interval and returns a stream of items.
/// This function runs indefinitely, as long as its polled.
fn repeat_query(
    mut monitor: BoxedMonitor,
    duration: tokio::time::Duration,
) -> impl Stream<Item = Result<StatusCode>> {
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
            match monitor.query().await {
                Ok(items) => {
                    for item in items {
                        yield Ok(item);
                    }
                },
                Err(err) => yield Err(err),
            }
        }
    }
}
