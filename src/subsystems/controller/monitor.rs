use std::time::Duration;

use async_trait::async_trait;
use miette::{Report, Result};
use tokio::{
    pin, select,
    sync::{
        broadcast::{self, Receiver, Sender},
        mpsc,
    },
    time::interval,
};
use tokio_graceful_shutdown::{IntoSubsystem, SubsystemBuilder, SubsystemHandle};
use tokio_stream::{Stream, StreamExt as _, wrappers::IntervalStream};

use crate::{
    MonitorSubsystem, adapters::BoxedMonitor, stats::Observation,
    subsystems::MONITOR_SUBSYSTEM_NAME,
};

/// The maximum number of observations that can be recevied before we
/// recompute statistical significance.
/// If this number is too low, we'll be performing compute-intensive
/// statical tests very often. If this number is too high, we could
/// be waiting too long before computing, which could permit us to promote more eagerly.
const DEFAULT_MAX_BATCH_SIZE: usize = 512;

const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);

pub const MONITOR_CONTROLLER_SUBSYSTEM_NAME: &str = "controller/monitor";

pub struct MonitorController<T: Observation> {
    monitor: BoxedMonitor<T>,
    sender: Sender<T>,
}

impl<T: Observation + Clone> MonitorController<T> {
    pub fn new(monitor: BoxedMonitor<T>) -> Self {
        let (sender, _) = broadcast::channel(DEFAULT_MAX_BATCH_SIZE);
        Self { monitor, sender }
    }

    pub fn subscribe(&self) -> Receiver<T> {
        self.sender.subscribe()
    }
}

#[async_trait]
impl<T: Observation + Send + 'static> IntoSubsystem<Report> for MonitorController<T> {
    async fn run(self, subsys: SubsystemHandle) -> Result<()> {
        let monitor_subsystem = MonitorSubsystem::new(self.monitor);
        let handle = monitor_subsystem.handle();
        let (shutdown_sender, mut shutdown_receiver) = mpsc::channel(1);

        // • Start the monitor subsystem.
        subsys.start(SubsystemBuilder::new(
            MONITOR_SUBSYSTEM_NAME,
            monitor_subsystem.into_subsystem(),
        ));

        tokio::spawn(async move {
            let events = repeat_query(handle, DEFAULT_INTERVAL);
            pin!(events);
            loop {
                select! {
                    _ = shutdown_receiver.recv() => {
                        return;
                    }
                    event = events.next() => {
                        match event {
                            None => break,
                            Some(e) => {
                                self.sender.send(e).unwrap();
                            },
                        }
                    }
                }
            }
        });

        subsys.on_shutdown_requested().await;
        shutdown_sender.send(()).await.unwrap();

        subsys.wait_for_children().await;
        Ok(())
    }
}

// TODO: Add a call to chunk_timeout to ensure that items are arriving after a particular
//       amount of time.
/// [repeat_query] runs the query on an interval and returns a stream of items.
/// This function runs indefinitely.
pub fn repeat_query<T: Observation>(
    mut observer: BoxedMonitor<T>,
    duration: tokio::time::Duration,
) -> impl Stream<Item = T> {
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
            let items = match observer.query().await {
                Err(err) => {
                    println!("Error: {}", err);
                    continue;
                },
                Ok(items) => items,
            };
            for item in items {
                yield item;
            }
        }
    }
}

/*
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
