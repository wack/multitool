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
//! order (completion vs declaration), never in per-requirement content.
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

/// Reserved height for the live region. Completed requirements flush out, so this
/// only ever needs to hold the in-flight tree + the gauge header.
const VIEWPORT_HEIGHT: u16 = 12;
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
            let lines = requirement_record_lines(&outcome, self.color);
            let height = lines.len() as u16;
            let _ = self.terminal.insert_before(height, move |buf| {
                let area = buf.area;
                Paragraph::new(lines).render(area, buf);
            });
            self.flushed.insert(req_index);
        }
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
        // A settle (or the discovery-complete gate opening) can complete a
        // requirement; flush it to scrollback immediately so the record is durable.
        if matches!(
            event,
            UiEvent::CheckSettled { .. } | UiEvent::DiscoveryComplete { .. }
        ) {
            self.flush_completed(state);
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

/// The persistent record for one completed requirement — byte-identical in text
/// to the reporting actor's output, only styled for the terminal.
fn requirement_record_lines(outcome: &RequirementOutcome, color: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let title = outcome.title.clone();
    lines.push(if color {
        let mut style = Style::new().add_modifier(Modifier::BOLD);
        style = style.fg(if outcome.satisfied {
            Color::Green
        } else {
            Color::Red
        });
        Line::styled(title, style)
    } else {
        let mark = if outcome.satisfied { "PASS" } else { "FAIL" };
        Line::raw(format!("[{mark}] {title}"))
    });
    if !outcome.satisfied {
        for check in outcome.failing_checks() {
            let text = failing_check_text(check);
            lines.push(if color {
                Line::styled(text, Style::new().fg(Color::Red))
            } else {
                Line::raw(text)
            });
        }
    }
    lines
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
    /// one record, never divergent.
    #[test]
    fn flushed_record_text_matches_reporting() {
        let failing = CheckOutcome {
            title: "c".into(),
            verdict: Verdict::Failed,
            evidence: Some("nope".into()),
        };
        let out = outcome("R", false, vec![failing.clone()]);

        let lines = requirement_record_lines(&out, false);
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
        let lines = requirement_record_lines(&out, false);
        assert_eq!(lines.len(), 1);
    }
}
