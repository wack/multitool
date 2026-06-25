//! Small pure formatters shared by the live backends (MULTI-1369): the spinner,
//! the two elapsed-time renderings, and the textual progress gauge. Kept pure and
//! unit-tested so the (visually un-testable) backends can lean on them.

use std::time::Duration;

/// Braille spinner frames, one per tick, for a Running check.
pub(crate) const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The spinner glyph for a given frame counter.
pub(crate) fn spinner_frame(frame: u64) -> &'static str {
    SPINNER[(frame as usize) % SPINNER.len()]
}

/// Compact elapsed time for the heartbeat header, e.g. `45s`, `3m12s`, `1h04m`.
pub(crate) fn human_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}h{m:02}m")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Clock-style elapsed time for a tree leaf, e.g. `0:12`, `3:05`, `1:02:09`.
pub(crate) fn clock_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A textual progress bar like `▕███████░░░░░▏`, `width` cells wide. With an
/// unknown or zero total the bar renders empty.
pub(crate) fn gauge_bar(done: usize, total: usize, width: usize) -> String {
    let filled = if total == 0 {
        0
    } else if done >= total {
        width
    } else {
        // Round to the nearest cell, but never report "full" until truly done.
        let exact = (done * width) as f64 / total as f64;
        (exact.round() as usize).min(width.saturating_sub(1))
    };
    let empty = width.saturating_sub(filled);
    format!("▕{}{}▏", "█".repeat(filled), "░".repeat(empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_renderings() {
        assert_eq!(human_elapsed(Duration::from_secs(45)), "45s");
        assert_eq!(human_elapsed(Duration::from_secs(192)), "3m12s");
        assert_eq!(human_elapsed(Duration::from_secs(3840)), "1h04m");
        assert_eq!(clock_elapsed(Duration::from_secs(12)), "0:12");
        assert_eq!(clock_elapsed(Duration::from_secs(185)), "3:05");
        assert_eq!(clock_elapsed(Duration::from_secs(3729)), "1:02:09");
    }

    #[test]
    fn gauge_fills_proportionally_and_caps() {
        assert_eq!(gauge_bar(0, 12, 12), "▕░░░░░░░░░░░░▏");
        assert_eq!(gauge_bar(12, 12, 12), "▕████████████▏");
        // Partial progress never shows a full bar.
        let bar = gauge_bar(11, 12, 12);
        assert!(bar.contains('░'), "in-progress bar must not be full: {bar}");
        // Unknown total ⇒ empty bar.
        assert_eq!(gauge_bar(3, 0, 6), "▕░░░░░░▏");
    }
}
