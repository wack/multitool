//! The TTY backend (MULTI-1369): a Ratatui **inline viewport**.
//!
//! The live requirement→check tree and progress gauge occupy a bounded region at
//! the bottom of the *normal* terminal buffer (never the alternate screen). As
//! each requirement's checks all settle, its rendered result line(s) are flushed
//! into permanent scrollback above the shrinking live region via
//! [`Terminal::insert_before`]. That one mechanism does triple duty — overflow
//! (the viewport only ever holds what's still in flight), liveness (results
//! appear durably the moment they're known), and persistence (by end-of-run the
//! full record already sits in scrollback, so there's **no separate final
//! reprint**). See the ticket for why this beats the alternate screen.
//!
//! The flushed record reuses the reporting actor's exact text (the requirement
//! line + [`failing_check_text`]) and the same AND-aggregation, so the TTY
//! scrollback and the non-TTY stdout report differ only in inter-requirement
//! order (completion vs declaration), never in per-requirement content. Unlike
//! [`report`], which prints through the real terminal and so gets its line
//! wrapping for free, a `ratatui` [`Buffer`] is a fixed-size grid: any row wider
//! than the buffer is silently clipped rather than wrapped. Because
//! `insert_before` renders into such a buffer, long evidence text (which
//! [`report`] never truncates) is word-wrapped to the terminal width *before*
//! it becomes [`Line`]s, so every wrapped row is reserved height and nothing is
//! lost.
//!
//! [`report`]: crate::checks::reporting::report
//! [`Buffer`]: ratatui::buffer::Buffer
//!
//! [`UiEvent::Log`]s get the same durable treatment: each one is flushed to
//! scrollback the instant it arrives ([`InlineTuiBackend::flush_log_line`]), via
//! the identical `insert_before` mechanism, so a `tracing::info!` mid-run can
//! never write raw bytes over the live region. The live region *also* keeps the
//! last few lines in a small pane below the tree — separate from it, never
//! interleaved into a check's row — purely as an ephemeral "what just happened"
//! glance; the scrollback copy is the permanent record.
//!
//! [`Terminal::insert_before`]: ratatui::Terminal::insert_before
//! [`failing_check_text`]: crate::checks::reporting::failing_check_text

use std::collections::HashSet;
use std::io::{self, Stdout, Write};
use std::sync::Once;
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::{cursor, execute};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::checks::model::{RequirementOutcome, Verdict};
use crate::checks::reporting::failing_check_text;

use super::UiEvent;
use super::backend::RenderBackend;
use super::format::{clock_elapsed, gauge_bar, human_elapsed, spinner_frame};
use super::state::{CheckState, PresenterState};

/// Reserved height for the live region. Completed requirements flush out, so
/// this only ever needs to hold the in-flight tree + the gauge header, plus a
/// few rows for the recent-log pane (header + up to `RECENT_LOGS_CAP` lines).
const VIEWPORT_HEIGHT: u16 = 16;
/// Width of the textual gauge bar.
const GAUGE_WIDTH: usize = 12;
/// Redraw cadence: ~20fps keeps the spinner and elapsed timers fluid.
const TICK: Duration = Duration::from_millis(50);

/// Inline-viewport TUI backend. Sole terminal writer for the whole TTY run.
pub(crate) struct InlineTuiBackend {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Whether to color the flushed record (honors `--enable-colors`).
    color: bool,
    /// Requirements already flushed to scrollback (so they leave the live tree).
    flushed: HashSet<usize>,
    /// Spinner frame counter, advanced each tick.
    frame: u64,
    /// Guards teardown idempotency.
    torn_down: bool,
}

impl InlineTuiBackend {
    /// Enter the inline viewport (hiding the cursor). Returns an `io::Error` if
    /// terminal setup fails, so the caller can fall back to the heartbeat.
    pub(crate) fn new(color: bool) -> io::Result<Self> {
        install_terminal_guards();
        let mut stdout = io::stdout();
        execute!(stdout, cursor::Hide)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(VIEWPORT_HEIGHT),
            },
        )?;
        Ok(Self {
            terminal,
            color,
            flushed: HashSet::new(),
            frame: 0,
            torn_down: false,
        })
    }

    /// The terminal's current column count — the exact width `insert_before`
    /// renders into (see the module docs), so this is what callers wrap text to
    /// before reserving height for it. Falls back to a sane default if the
    /// terminal can't report its size (never observed in practice).
    fn terminal_width(&self) -> u16 {
        self.terminal.size().map(|s| s.width).unwrap_or(80)
    }

    /// Flush every requirement that has fully settled (and isn't already flushed)
    /// into scrollback, in `req_index` order. Only runs once discovery is complete
    /// so a requirement's check count is final.
    fn flush_completed(&mut self, state: &PresenterState) {
        if !state.discovery_complete {
            return;
        }
        for req_index in state.requirement_indices() {
            if self.flushed.contains(&req_index) || !state.requirement_complete(req_index) {
                continue;
            }
            let Some(outcome) = state.requirement_outcome(req_index) else {
                continue;
            };
            // Word-wrap to the render width now: each wrapped row becomes its own
            // `Line`, which makes `lines.len()` the correct height to reserve —
            // no row gets clipped.
            let width = self.terminal_width();
            let lines = requirement_record_lines(&outcome, self.color, width);
            let height = lines.len() as u16;
            let _ = self.terminal.insert_before(height, move |buf| {
                let area = buf.area;
                Paragraph::new(lines).render(area, buf);
            });
            self.flushed.insert(req_index);
        }
    }

    /// Flush a single routed log line into scrollback immediately, the same way
    /// a completed requirement is flushed — so it never fights the live region
    /// for control of the cursor, and (per the module docs) is word-wrapped
    /// rather than clipped. This is the *permanent* record of the line; the live
    /// region separately keeps the last few for at-a-glance context (see
    /// [`live_lines`]).
    fn flush_log_line(&mut self, line: &str) {
        let width = self.terminal_width();
        let style = self.color.then(|| Style::new().add_modifier(Modifier::DIM));
        let lines = styled_wrapped_lines(line, width, style);
        let height = lines.len() as u16;
        let _ = self.terminal.insert_before(height, move |buf| {
            let area = buf.area;
            Paragraph::new(lines).render(area, buf);
        });
    }

    /// Redraw the live region (gauge header + in-flight tree).
    fn draw_live(&mut self, state: &PresenterState) {
        let lines = live_lines(state, &self.flushed, self.frame, GAUGE_WIDTH);
        let _ = self.terminal.draw(|f| {
            let area = f.area();
            f.render_widget(Paragraph::new(lines), area);
        });
    }
}

impl RenderBackend for InlineTuiBackend {
    fn apply(&mut self, state: &PresenterState, event: &UiEvent) {
        if self.torn_down {
            return;
        }
        match event {
            // A settle (or the discovery-complete gate opening) can complete a
            // requirement; flush it to scrollback immediately so the record is
            // durable.
            UiEvent::CheckSettled { .. } | UiEvent::DiscoveryComplete { .. } => {
                self.flush_completed(state);
            }
            UiEvent::Log(line) => self.flush_log_line(line),
            _ => {}
        }
    }

    fn tick(&mut self, state: &PresenterState) {
        if self.torn_down {
            return;
        }
        self.frame = self.frame.wrapping_add(1);
        self.draw_live(state);
    }

    fn teardown(&mut self, state: &PresenterState) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        // Flush any stragglers (by RunComplete all requirements have settled).
        self.flush_completed(state);
        // Mirror the reporting actor's empty-suite line.
        if state.rows.is_empty() {
            let _ = self.terminal.insert_before(1, |buf| {
                let area = buf.area;
                Paragraph::new(Line::raw("No requirements found.")).render(area, buf);
            });
        }
        // Remove the live viewport, then restore the cursor.
        let _ = self.terminal.clear();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, cursor::Show);
        let _ = stdout.flush();
    }

    fn tick_interval(&self) -> Duration {
        TICK
    }
}

/// The escape sequence that shows the cursor (`CSI ? 25 h`).
const SHOW_CURSOR: &[u8] = b"\x1b[?25h";

/// Restore the cursor on the two paths that bypass `on_stop` — a panic and a
/// SIGINT (Ctrl-C). Normal/error exits restore it in [`InlineTuiBackend::teardown`]
/// via `on_stop`. Installed once.
fn install_terminal_guards() {
    static GUARDS: Once = Once::new();
    GUARDS.call_once(|| {
        // Panic: show the cursor, then chain to the previous hook.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let mut stdout = io::stdout();
            let _ = execute!(stdout, cursor::Show);
            let _ = stdout.flush();
            prev(info);
        }));

        // SIGINT: the default handler would terminate immediately, leaving the
        // cursor hidden. Replace it with an async-signal-safe restore-and-exit.
        // SAFETY: `restore_on_sigint` only calls async-signal-safe libc functions
        // (`write`, `_exit`).
        unsafe {
            libc::signal(
                libc::SIGINT,
                restore_on_sigint as *const () as libc::sighandler_t,
            );
        }
    });
}

/// Async-signal-safe SIGINT handler: write the show-cursor sequence straight to
/// the stdout fd and exit 130 (the conventional 128 + SIGINT code).
extern "C" fn restore_on_sigint(_sig: libc::c_int) {
    // SAFETY: `write` and `_exit` are async-signal-safe; the buffer is static.
    unsafe {
        libc::write(
            libc::STDOUT_FILENO,
            SHOW_CURSOR.as_ptr().cast(),
            SHOW_CURSOR.len(),
        );
        libc::_exit(130);
    }
}

/// The persistent record for one completed requirement — same text content as
/// the reporting actor's output (just spread across more rows when a line is
/// word-wrapped to `width`), styled for the terminal.
fn requirement_record_lines(
    outcome: &RequirementOutcome,
    color: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let title_style = color.then(|| {
        Style::new()
            .add_modifier(Modifier::BOLD)
            .fg(if outcome.satisfied {
                Color::Green
            } else {
                Color::Red
            })
    });
    let title_text = if color {
        outcome.title.clone()
    } else {
        let mark = if outcome.satisfied { "PASS" } else { "FAIL" };
        format!("[{mark}] {}", outcome.title)
    };
    lines.extend(styled_wrapped_lines(&title_text, width, title_style));

    if !outcome.satisfied {
        let evidence_style = color.then(|| Style::new().fg(Color::Red));
        for check in outcome.failing_checks() {
            let text = failing_check_text(check);
            lines.extend(styled_wrapped_lines(&text, width, evidence_style));
        }
    }
    lines
}

/// Word-wrap `text` to `width` columns and render each resulting row as its own
/// `Line`, uniformly styled. Wrapping (rather than clipping) is what keeps long
/// evidence text from being lost off the edge of the fixed-size `Buffer` that
/// [`InlineTuiBackend::flush_completed`] renders into.
fn styled_wrapped_lines(text: &str, width: u16, style: Option<Style>) -> Vec<Line<'static>> {
    // A degenerate width (terminal not yet sized) can't wrap meaningfully;
    // fall back to a single unwrapped row rather than looping or dividing by it.
    if width == 0 {
        return vec![raw_or_styled(text.to_string(), style)];
    }
    textwrap::wrap(text, width as usize)
        .into_iter()
        .map(|row| raw_or_styled(row.into_owned(), style))
        .collect()
}

fn raw_or_styled(text: String, style: Option<Style>) -> Line<'static> {
    match style {
        Some(style) => Line::styled(text, style),
        None => Line::raw(text),
    }
}

/// Build the live region: the gauge/tally header, then the in-flight tree grouped
/// by requirement (flushed requirements excluded). Requirements with active work
/// are ordered first so a bounded viewport never hides a running check.
fn live_lines(
    state: &PresenterState,
    flushed: &HashSet<usize>,
    frame: u64,
    gauge_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let done = state.done();
    let total = state.total.unwrap_or(0);
    let (sat, failed, errored) = state.verdict_tallies();
    let bar = gauge_bar(done, total, gauge_width);
    let total_str = state
        .total
        .map(|t| t.to_string())
        .unwrap_or_else(|| "?".to_string());
    let elapsed = human_elapsed(state.run_started.elapsed());
    lines.push(Line::raw(format!(
        "checks  {bar} {done}/{total_str}   ✓{sat}  ✗{failed}  ⚠{errored}   · {} running · {elapsed}",
        state.running(),
    )));

    // Order: requirements with any running/retrying check first (liveness), then
    // by declaration order. Flushed requirements are gone from the tree.
    let mut reqs: Vec<usize> = state
        .requirement_indices()
        .into_iter()
        .filter(|r| !flushed.contains(r))
        .collect();
    reqs.sort_by_key(|&r| {
        let active = state
            .requirement_rows(r)
            .iter()
            .any(|row| matches!(row.state, CheckState::Running | CheckState::Retrying(_)));
        (!active, r)
    });

    for (i, &req_index) in reqs.iter().enumerate() {
        let rows = state.requirement_rows(req_index);
        let req_title = rows
            .first()
            .map(|r| r.req_title.clone())
            .unwrap_or_default();
        let settled = rows
            .iter()
            .filter(|r| matches!(r.state, CheckState::Settled(_)))
            .count();
        let last_req = i + 1 == reqs.len();
        let parent_branch = if last_req { "└─ " } else { "├─ " };
        lines.push(Line::raw(format!(
            "{parent_branch}{req_title}  ({settled}/{})",
            rows.len()
        )));

        let cont = if last_req { "   " } else { "│  " };
        for (j, row) in rows.iter().enumerate() {
            let last_child = j + 1 == rows.len();
            let child_branch = if last_child { "└─ " } else { "├─ " };
            let glyph = leaf_glyph(&row.state, frame);
            let label = format!("{cont}{child_branch}{glyph} {}", row.check_title);
            let elapsed = row
                .started
                .map(|s| clock_elapsed(s.elapsed()))
                .unwrap_or_default();
            lines.push(Line::raw(format!("{label:<44}{elapsed}")));
        }
    }

    // A small, separate pane for the most recent log lines — kept apart from
    // the tree above so a mid-run `tracing::info!` never reads as one of its
    // rows. The full history of every line already sits in scrollback (each is
    // flushed there the instant it arrives; see `flush_log_line`), so this is
    // just an ephemeral "what just happened" glance, not the record of truth.
    if !state.recent_logs.is_empty() {
        lines.push(Line::raw("logs"));
        for entry in &state.recent_logs {
            lines.push(Line::raw(format!("  {entry}")));
        }
    }

    lines
}

/// The leaf glyph for a check: spinner while active, verdict mark once settled.
fn leaf_glyph(state: &CheckState, frame: u64) -> String {
    match state {
        CheckState::Queued => "·".to_string(),
        CheckState::Running | CheckState::Retrying(_) => spinner_frame(frame).to_string(),
        CheckState::Settled(Verdict::Satisfied) => "✓".to_string(),
        CheckState::Settled(Verdict::Failed) => "✗".to_string(),
        CheckState::Settled(Verdict::Errored) => "⚠".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::model::CheckOutcome;
    use crate::checks::reporting::{failing_check_text as report_text, format_requirement_plain};

    fn outcome(title: &str, satisfied: bool, checks: Vec<CheckOutcome>) -> RequirementOutcome {
        RequirementOutcome {
            title: title.into(),
            filepath: std::path::PathBuf::new(),
            satisfied,
            check_outcomes: checks,
        }
    }

    /// The flushed record's plain text must match the reporting actor's output —
    /// one record, never divergent — as long as the width is generous enough
    /// that nothing wraps.
    #[test]
    fn flushed_record_text_matches_reporting() {
        let failing = CheckOutcome {
            title: "c".into(),
            verdict: Verdict::Failed,
            evidence: Some("nope".into()),
        };
        let out = outcome("R", false, vec![failing.clone()]);

        let lines = requirement_record_lines(&out, false, 80);
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert_eq!(rendered[0], format_requirement_plain(&out));
        assert_eq!(rendered[1], report_text(&failing));
    }

    #[test]
    fn satisfied_record_is_single_line() {
        let out = outcome("OK", true, vec![]);
        let lines = requirement_record_lines(&out, false, 80);
        assert_eq!(lines.len(), 1);
    }

    /// Evidence text longer than the terminal is word-wrapped into multiple
    /// rows rather than being clipped by the fixed-width `Buffer` — the fix for
    /// long error messages getting cut off once the TUI hands off to scrollback.
    #[test]
    fn long_evidence_wraps_instead_of_clipping() {
        // Every word is short enough to fit within `width` on its own, so
        // wrapping only ever splits *between* words, never inside one — which
        // keeps the "every word survives" assertion below unambiguous.
        let evidence = "this evidence line is much longer than the width so it wraps across several rows instead of clipping";
        let failing = CheckOutcome {
            title: "c".into(),
            verdict: Verdict::Failed,
            evidence: Some(evidence.into()),
        };
        let out = outcome("R", false, vec![failing]);

        let width = 10;
        let lines = requirement_record_lines(&out, false, width);
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        // No row exceeds the reserved width...
        assert!(
            rendered
                .iter()
                .all(|row| row.chars().count() <= width as usize)
        );
        // ...and every word of the original evidence survives somewhere in the
        // wrapped output, so nothing was silently dropped.
        let joined = rendered.join(" ");
        for word in evidence.split_whitespace() {
            assert!(joined.contains(word), "lost {word:?} from wrapped evidence");
        }
    }
}
