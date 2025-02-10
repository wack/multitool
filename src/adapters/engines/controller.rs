use async_stream::stream;
use futures_core::Stream;
use tokio::{pin, select, time::interval};
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt;

use crate::stats::Observation;

use super::{Action, DecisionEngine};

// TODO: This type is not currently used, and I haven't really
//       incorporated it into my mental model of the Pipeline.
//       I think it's supposed to wrap the monitor and control
//       configurables, but I'm not planning to bubble those
//       configurables up to the CLI until a future release.
/// An [EngineController] is a wrapper around a DecisionEngine that
/// controls how and when its called. It essentially converts the
/// [DecisionEngine] into an async stream that only emits [Action]s
/// when there's an action to take.
pub struct EngineController {
    // TODO: Implement these fields.
    // Only run engine if this many samples
    // has been received.
    // minimum_samples: u64,
    // Always run the engine if this many samples has been received.
    // maximum_samples: u64,
    // If this amount of time has elapsed, and the minimum number of samples
    // has been received, then run the engine.
    // minimum_duration: tokio::time::Duration,
    // If this amount of time has elapsed, run the engine even if the
    // minimum number of samples has not yet been reached.
    maximum_duration: tokio::time::Duration,
    // receive a shutdown signal.
    // shutdown: Receiver<()>,
}

impl EngineController {
    pub fn new(maximum_duration: tokio::time::Duration) -> Self {
        Self { maximum_duration }
    }

    /// Convert this controller into a stream that emits [Action]s from the Engine.
    pub fn run<T: Observation>(
        self,
        mut engine: Box<dyn DecisionEngine<T>>,
        observations: impl Stream<Item = Vec<T>>,
    ) -> impl Stream<Item = Action> {
        stream! {
            // TODO: Set the MissedTickBehavior on the internal.
            // TODO: Implement the stream controls.
            let timer = IntervalStream::new(interval(self.maximum_duration));
            // Pin our streams to the stack for iteration.
            pin!(timer);
            pin!(observations);

            /*
            TODO: it looks like yield cannot be used from within a closure. Consider
            // verifying and filing a bug if that's the case.
            // A helper with yield syntax. This is how we run the engine, dumping
            // an item to the stream only if its actionable.
            let compute_next = || {
                if let Some(action) = engine.compute() {
                    yield action;
                }
            };
            */

            // • Check to see if we can read a new observation.
            loop {
                select! {
                    _ = timer.next() => {
                        // • Timer has ticked! Run the engine and check for the results.
                        // compute_next();
                        if let Some(action) = engine.compute() {
                            yield action;
                        }
                    }
                    observation = observations.next() => {
                        match observation {
                            Some(obs) => {
                                for observ in obs {
                                    engine.add_observation(observ);
                                }
                            },
                            // Nothing left for us to compute.
                            // Run the engine one last time and exit.
                            None => {
                                // compute_next();
                                if let Some(action) = engine.compute() {
                                    yield action;
                                }
                                break;
                            },
                        }
                    }
                }
            }
        }
    }
}
