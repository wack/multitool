//! Headless backends (MULTI-1369).
//!
//! [`NullBackend`] discards everything — the default for the execution/reporting
//! tests that drive the pipeline but don't care about presentation. The future
//! `--quiet` flag will select it too. [`RecordingBackend`] keeps every applied
//! [`UiEvent`] in a shared buffer so a test can assert on the milestone stream
//! without a terminal.

use std::time::Duration;

#[cfg(test)]
use std::sync::{Arc, Mutex};

use super::UiEvent;
use super::backend::RenderBackend;
use super::state::PresenterState;

/// A no-op backend: the events flow, nothing is rendered. Spawned for every
/// pipeline test (and, later, for `--quiet`).
pub(crate) struct NullBackend;

impl RenderBackend for NullBackend {
    fn apply(&mut self, _state: &PresenterState, _event: &UiEvent) {}
    fn tick(&mut self, _state: &PresenterState) {}
    fn teardown(&mut self, _state: &PresenterState) {}
    fn tick_interval(&self) -> Duration {
        // Long: a no-op backend never needs to wake up to redraw.
        Duration::from_secs(3600)
    }
}

/// A backend that records the [`UiEvent`]s it is handed, for test assertions.
#[cfg(test)]
pub(crate) struct RecordingBackend {
    events: Arc<Mutex<Vec<UiEvent>>>,
}

#[cfg(test)]
impl RecordingBackend {
    /// Build a backend plus the shared handle a test reads the events back from.
    pub(crate) fn new() -> (Self, Arc<Mutex<Vec<UiEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                events: events.clone(),
            },
            events,
        )
    }
}

#[cfg(test)]
impl RenderBackend for RecordingBackend {
    fn apply(&mut self, _state: &PresenterState, event: &UiEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
    fn tick(&mut self, _state: &PresenterState) {}
    fn teardown(&mut self, _state: &PresenterState) {}
    fn tick_interval(&self) -> Duration {
        Duration::from_secs(3600)
    }
}
