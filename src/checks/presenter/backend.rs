//! The polymorphic seam (MULTI-1369).
//!
//! Events are a concrete [`UiEvent`] enum; the *backend* is where rendering
//! polymorphism lives. The presenter holds a `Box<dyn RenderBackend>` chosen at
//! startup from whether stdout is a TTY:
//!
//! * [`InlineTuiBackend`] — stdout is a TTY: a Ratatui inline viewport.
//! * [`HeartbeatBackend`] — not a TTY: a periodic stderr progress line.
//! * [`NullBackend`] / [`RecordingBackend`] — tests: no terminal.
//!
//! [`UiEvent`]: super::UiEvent
//! [`InlineTuiBackend`]: super::inline::InlineTuiBackend
//! [`HeartbeatBackend`]: super::heartbeat::HeartbeatBackend
//! [`NullBackend`]: super::recording::NullBackend
//! [`RecordingBackend`]: super::recording::RecordingBackend

use std::time::Duration;

use super::UiEvent;
use super::state::PresenterState;

/// A render target for the live presenter. Implementations are display-only:
/// they read [`PresenterState`] + the latest [`UiEvent`] and surface progress;
/// they never feed input back into the pipeline.
pub(crate) trait RenderBackend: Send {
    /// React to a single event *after* it has been folded into `state`. Used for
    /// event-driven work (e.g. flushing a completed requirement to scrollback);
    /// state-derived rendering belongs in [`RenderBackend::tick`].
    fn apply(&mut self, state: &PresenterState, event: &UiEvent);

    /// Redraw on the periodic clock — this is what keeps spinners and the
    /// elapsed-time counters moving between events (the liveness signal).
    fn tick(&mut self, state: &PresenterState);

    /// Final cleanup: flush any pending record, clear ephemeral UI, and restore
    /// the terminal. Must be idempotent — it runs from the actor's `on_stop`.
    fn teardown(&mut self, state: &PresenterState);

    /// How often [`RenderBackend::tick`] should fire. Fast for the TUI (smooth
    /// spinners); slow for the heartbeat (one tidy line every several seconds).
    fn tick_interval(&self) -> Duration;
}
