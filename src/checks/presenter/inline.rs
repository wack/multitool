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

use crate::checks::model::{DecidedBy, RequirementOutcome, Verdict, decided_by_summary};
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
        let lines = live_lines(state, &self.flushed, self.frame, GAUGE_WIDTH, self.color);
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
///
/// `pub(crate)` (not private): `checks::plan`'s own inline backend
/// (MULTI-1829, `--features jev`) also opens an inline viewport and needs the
/// exact same guards — reused rather than duplicated (see
/// `checks::presenter::mod`'s re-export). Safe to call from both: the `Once`
/// makes a second call a no-op, and only one of `multi check`/`multi plan`
/// ever runs per process.
pub(crate) fn install_terminal_guards() {
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
///
/// `pub(crate)` (not private): shared with `checks::plan`'s own inline
/// backend — see [`install_terminal_guards`]'s doc comment for why.
pub(crate) fn styled_wrapped_lines(
    text: &str,
    width: u16,
    style: Option<Style>,
) -> Vec<Line<'static>> {
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
///
/// `color` (MULTI-1827) governs only the extra dim styling a `Cached` row's
/// skip glyph gets (see [`row_style`]); it does not touch anything else in
/// this view — the header, the tree's structure/text, and every `Agent` row
/// are rendered identically regardless, exactly as before this ticket.
fn live_lines(
    state: &PresenterState,
    flushed: &HashSet<usize>,
    frame: u64,
    gauge_width: usize,
    color: bool,
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
    let mut header = format!(
        "checks  {bar} {done}/{total_str}   ✓{sat}  ✗{failed}  ⚠{errored}   · {} running · {elapsed} · {}",
        state.running(),
        state.model,
    );
    // MULTI-1827: `N cached · N jev · N agent`, appended the instant the
    // first non-agent-decided check settles (see `decided_by_summary`'s
    // docs) — never in the default build, where every check is
    // `DecidedBy::Agent` and this stays `None`.
    let (cached, jev, agent) = state.decided_by_tallies();
    if let Some(summary) = decided_by_summary(cached, jev, agent) {
        header.push_str(&format!(" · {summary}"));
    }
    lines.push(Line::raw(header));

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
            // MULTI-1827: a settled row's decider — `None` until settled, and
            // `Some(DecidedBy::Agent)` for every check in the default build,
            // which `leaf_glyph`/`DecidedBy::tag` both treat exactly like `None`.
            let decided_by = row.outcome.as_ref().map(|o| o.decided_by);
            let glyph = leaf_glyph(&row.state, decided_by, frame);
            let mut title = row.check_title.clone();
            if let Some(tag) = decided_by.and_then(DecidedBy::tag) {
                title.push_str(&format!(" [{tag}]"));
            }
            let label = format!("{cont}{child_branch}{glyph} {title}");
            let elapsed = row
                .started
                .map(|s| clock_elapsed(s.elapsed()))
                .unwrap_or_default();
            let mut text = format!("{label:<44}{elapsed}");
            // MULTI-1828: a running row also shows which turn its agent is on
            // and, once one has arrived, its most recent allowlisted tool
            // call. The live region is a fixed-width `Buffer` (see the module
            // docs), so a long line is clipped rather than wrapped — that's
            // the "truncated to the row width" the ticket asks for, for free.
            if matches!(row.state, CheckState::Running)
                && let Some(progress) = &row.progress
            {
                text.push_str(&format!("  turn {}/{}", progress.turn, progress.max_turns));
                if let Some(activity) = &progress.activity {
                    text.push_str(&format!(" · {activity}"));
                }
            }
            lines.push(match row_style(color, &row.state, decided_by) {
                Some(style) => Line::styled(text, style),
                None => Line::raw(text),
            });
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

/// The glyph for a check settled from the cache (MULTI-1827): distinct from
/// every verdict mark below so a skipped check — no agent or Jev call at
/// all — never reads as one an agent or Jev actually decided.
const SKIP_GLYPH: &str = "⏭";

/// The leaf glyph for a check: spinner while active, verdict mark once
/// settled — except a check settled from the cache (MULTI-1827), which
/// always shows [`SKIP_GLYPH`] instead of the verdict mark, since the point
/// is that *nothing ran* to decide it. `Jev` keeps the ordinary verdict mark
/// (only its tag, appended by the caller, says how it was decided); `Agent`
/// (`None`, or `Some(DecidedBy::Agent)`) is unchanged from before this
/// ticket.
fn leaf_glyph(state: &CheckState, decided_by: Option<DecidedBy>, frame: u64) -> String {
    if decided_by == Some(DecidedBy::Cached) {
        return SKIP_GLYPH.to_string();
    }
    match state {
        CheckState::Queued => "·".to_string(),
        CheckState::Running | CheckState::Retrying(_) => spinner_frame(frame).to_string(),
        CheckState::Settled(Verdict::Satisfied) => "✓".to_string(),
        CheckState::Settled(Verdict::Failed) => "✗".to_string(),
        CheckState::Settled(Verdict::Errored) => "⚠".to_string(),
    }
}

/// The live-tree row style for a settled check (MULTI-1827): `None` — i.e.
/// [`Line::raw`], byte-for-byte what this row rendered before this ticket —
/// for every `Agent` row (the only kind in the default build) and whenever
/// color is disabled. A `Cached` row otherwise gets a dim green/red matching
/// its verdict: dim because nothing actually ran to decide it, but still
/// colored by verdict so [`SKIP_GLYPH`] replacing the usual `✓`/`✗` doesn't
/// cost the reader the pass/fail distinction the old glyph carried alone.
/// `Jev` rows are left unstyled — only their tag marks them as non-agent.
fn row_style(color: bool, state: &CheckState, decided_by: Option<DecidedBy>) -> Option<Style> {
    if !color || decided_by != Some(DecidedBy::Cached) {
        return None;
    }
    let CheckState::Settled(verdict) = state else {
        return None;
    };
    let fg = if verdict.is_satisfied() {
        Color::Green
    } else {
        Color::Red
    };
    Some(Style::new().fg(fg).add_modifier(Modifier::DIM))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::model::{CheckOutcome, DecidedBy};
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
            decided_by: DecidedBy::Agent,
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

    /// The live header names the model in use, so a run against a non-default
    /// provider/model is visibly distinguishable from one against the default.
    #[test]
    fn header_shows_the_model() {
        let state = PresenterState::new("claude-sonnet-4-6".into());
        let lines = live_lines(&state, &HashSet::new(), 0, GAUGE_WIDTH, false);
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(header.ends_with("claude-sonnet-4-6"), "{header}");
    }

    /// MULTI-1828 acceptance: a running row shows `turn N/max · <activity>`.
    #[test]
    fn running_row_shows_turn_and_activity() {
        let mut state = PresenterState::new("test-model".into());
        state.apply(&UiEvent::CheckQueued {
            id: 0,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
        state.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });
        state.apply(&UiEvent::CheckProgress {
            id: 0,
            attempt: 1,
            turn: 3,
            max_turns: 30,
            activity: Some("Read src/auth/sign.rs".into()),
        });

        let lines = live_lines(&state, &HashSet::new(), 0, GAUGE_WIDTH, false);
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            rendered
                .iter()
                .any(|l| l.contains("turn 3/30 · Read src/auth/sign.rs")),
            "{rendered:?}"
        );
    }

    /// A running row with no progress yet (no `TurnStart` observed) shows no
    /// turn/activity suffix at all — nothing to show yet.
    #[test]
    fn running_row_without_progress_shows_no_turn_suffix() {
        let mut state = PresenterState::new("test-model".into());
        state.apply(&UiEvent::CheckQueued {
            id: 0,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
        state.apply(&UiEvent::CheckStarted { id: 0, attempt: 1 });

        let lines = live_lines(&state, &HashSet::new(), 0, GAUGE_WIDTH, false);
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(!rendered.iter().any(|l| l.contains("turn")), "{rendered:?}");
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
            decided_by: DecidedBy::Agent,
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

    // -- MULTI-1827: decided-by rendering in the live tree -----------------

    fn queued(state: &mut PresenterState, id: usize) {
        state.apply(&UiEvent::CheckQueued {
            id,
            req_index: 0,
            req_title: "R".into(),
            check_title: "c".into(),
        });
    }

    fn settle(state: &mut PresenterState, id: usize, verdict: Verdict, decided_by: DecidedBy) {
        state.apply(&UiEvent::CheckSettled {
            id,
            outcome: CheckOutcome {
                title: "c".into(),
                verdict,
                evidence: None,
                decided_by,
            },
        });
    }

    fn rendered(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// One requirement, one check, freshly discovered and settled: `live_lines`
    /// always produces exactly `[header, requirement line, check row]` — this is
    /// the check row's index. (Searching for it by glyph is unreliable: the
    /// header's own `✓{sat} ✗{failed}` tally can contain the very glyph a test
    /// is looking for — see the MULTI-1827 code-review note below.)
    const CHECK_ROW: usize = 2;

    /// MULTI-1827 acceptance: a `Cached` row shows the distinct skip glyph
    /// (never the ordinary `✓`) and a `cached` tag next to its title.
    #[test]
    fn cached_row_shows_skip_glyph_and_cached_tag() {
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        settle(&mut state, 0, Verdict::Satisfied, DecidedBy::Cached);

        let lines = live_lines(&state, &HashSet::new(), 0, GAUGE_WIDTH, false);
        let rows = rendered(&lines);
        assert_eq!(rows.len(), 3, "{rows:?}");
        let row = &rows[CHECK_ROW];
        assert!(row.contains(SKIP_GLYPH), "{row}");
        assert!(row.contains("[cached]"), "{row}");
        assert!(!row.contains('✓'), "{row}");
    }

    /// MULTI-1827 acceptance: a `Jev` row keeps the ordinary verdict glyph
    /// (here `✗`, since the check failed) and gets a `jev` tag.
    ///
    /// Code-review note: an earlier version of this test located the row by
    /// searching for `'✗'`, which — with exactly one failing check — also
    /// matches the *header*'s own `✗1` tally (`rows[0]`), so the assertion
    /// passed for the wrong reason. Indexing [`CHECK_ROW`] directly closes
    /// that hole.
    #[test]
    fn jev_row_keeps_verdict_glyph_and_gets_jev_tag() {
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        settle(&mut state, 0, Verdict::Failed, DecidedBy::Jev);

        let lines = live_lines(&state, &HashSet::new(), 0, GAUGE_WIDTH, false);
        let rows = rendered(&lines);
        assert_eq!(rows.len(), 3, "{rows:?}");
        let row = &rows[CHECK_ROW];
        assert!(row.contains('✗'), "{row}");
        assert!(row.contains("[jev]"), "{row}");
        assert!(!row.contains(SKIP_GLYPH), "{row}");
    }

    /// MULTI-1827 acceptance: an `Agent` row and the header — the only kind
    /// of row in the default build — render BYTE-FOR-BYTE what they rendered
    /// before this ticket. The expected strings below were captured running
    /// this exact scenario (`PresenterState::new("test-model".into())`, one
    /// queued check titled "c" under requirement "R", settled `Satisfied`)
    /// against `live_lines` on the parent commit, `robbie/multi-1826`
    /// (pre-MULTI-1827): a scratch `git worktree add <path> robbie/multi-1826`,
    /// a temporary test printing the exact same scenario's rendered rows,
    /// `cargo test -- --nocapture` to capture them, then the worktree was
    /// removed — no commit was made there.
    ///
    /// A prior version of this test only asserted the absence of
    /// `cached`/`jev` substrings, which would still pass if an Agent row
    /// picked up stray trailing whitespace or any other spacing drift —
    /// exact string equality (including the padding spaces the `{:<44}`
    /// column width already added before this ticket) closes that hole.
    ///
    /// It must compare full lines rather than search for `'✓'`: with one
    /// satisfied check the header's own tally also reads `✓1`, so a naive
    /// substring search matches the header (`rows[0]`) first, not the check
    /// row.
    #[test]
    fn agent_row_and_header_render_identically_to_before() {
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        settle(&mut state, 0, Verdict::Satisfied, DecidedBy::Agent);

        let lines = live_lines(&state, &HashSet::new(), 0, GAUGE_WIDTH, true);
        let rows = rendered(&lines);
        assert_eq!(
            rows,
            vec![
                "checks  ▕░░░░░░░░░░░░▏ 1/?   ✓1  ✗0  ⚠0   · 0 running · 0s · test-model"
                    .to_string(),
                "└─ R  (1/1)".to_string(),
                "   └─ ✓ c                                   ".to_string(),
            ]
        );
        assert_eq!(
            lines[CHECK_ROW].style,
            Style::default(),
            "agent row must be unstyled (Line::raw), byte-for-byte as before"
        );
    }

    /// MULTI-1827 acceptance: color enabled, a `Cached` row's line style is a
    /// dim green when satisfied and a dim red when failed — "still green/red
    /// by verdict" even though the glyph itself no longer varies by verdict.
    #[test]
    fn cached_row_style_is_dim_and_colored_by_verdict() {
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        queued(&mut state, 1);
        settle(&mut state, 0, Verdict::Satisfied, DecidedBy::Cached);
        settle(&mut state, 1, Verdict::Failed, DecidedBy::Cached);

        let lines = live_lines(&state, &HashSet::new(), 0, GAUGE_WIDTH, true);
        // Two requirement-tree rows beyond the header + the requirement line.
        let styles: Vec<Style> = lines.iter().map(|l| l.style).collect();
        assert!(
            styles.contains(&Style::new().fg(Color::Green).add_modifier(Modifier::DIM)),
            "{styles:?}"
        );
        assert!(
            styles.contains(&Style::new().fg(Color::Red).add_modifier(Modifier::DIM)),
            "{styles:?}"
        );
    }

    /// MULTI-1827 acceptance ("respect the existing no-color path"): the
    /// `cached`/`jev` tags are literal text, present whether or not color is
    /// enabled — never conveyed only through the `Cached` row's dim styling.
    #[test]
    fn tags_are_plain_text_regardless_of_color() {
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        queued(&mut state, 1);
        settle(&mut state, 0, Verdict::Satisfied, DecidedBy::Cached);
        settle(&mut state, 1, Verdict::Satisfied, DecidedBy::Jev);

        for color in [false, true] {
            let lines = live_lines(&state, &HashSet::new(), 0, GAUGE_WIDTH, color);
            let joined = rendered(&lines).join("\n");
            assert!(joined.contains("[cached]"), "color={color}: {joined}");
            assert!(joined.contains("[jev]"), "color={color}: {joined}");
        }
    }

    /// MULTI-1827 acceptance: the header's `N cached · N jev · N agent`
    /// summary appears the instant the first non-agent-decided check
    /// settles, not only once every check has, and is absent altogether for
    /// an all-`Agent` run.
    #[test]
    fn header_gains_decider_summary_as_soon_as_a_non_agent_check_settles() {
        let mut state = PresenterState::new("test-model".into());
        queued(&mut state, 0);
        queued(&mut state, 1);

        let header = |s: &PresenterState| {
            rendered(&live_lines(s, &HashSet::new(), 0, GAUGE_WIDTH, false))[0].clone()
        };
        assert!(!header(&state).contains("cached"), "{}", header(&state));

        settle(&mut state, 0, Verdict::Satisfied, DecidedBy::Cached);
        let h = header(&state);
        assert!(h.contains("1 cached · 0 jev · 0 agent"), "{h}");

        settle(&mut state, 1, Verdict::Satisfied, DecidedBy::Agent);
        let h = header(&state);
        assert!(h.contains("1 cached · 0 jev · 1 agent"), "{h}");
    }
}
