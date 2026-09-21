//! The TTY backend for `multi plan` (MULTI-1829) — the plan-side counterpart
//! of `crate::checks::presenter::inline`. A Ratatui **inline viewport**
//! identical in mechanics (never the alternate screen; the cursor-restore
//! guards are the exact same installation — see
//! [`crate::checks::presenter::install_terminal_guards`]), but simplified on
//! purpose: `multi plan` only ever re-runs an agent for a *stale* check
//! (typically a small minority of the suite), so — unlike `multi check`'s
//! backend, which flushes each requirement to scrollback the moment it
//! completes to bound a large live tree — this backend renders the live tree
//! in place for the whole run and flushes the **entire** final record once,
//! at [`PlanInlineTuiBackend::teardown`]. That is a deliberate scope
//! reduction from the check presenter's incremental-flush design, not an
//! oversight — see this ticket's final report for the trade-off.

use std::io::{self, Stdout, Write};
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::{cursor, execute};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::checks::jev::plan_file::AgentReason;
use crate::checks::plan::agent_reason_str;
use crate::checks::presenter::format::{clock_elapsed, gauge_bar, human_elapsed, spinner_frame};
use crate::checks::presenter::{install_terminal_guards, styled_wrapped_lines};

use super::PlanRenderBackend;
use super::PlanUiEvent;
use super::state::{PlanPresenterState, PlanRow, PlanRowState, StaleReason};

/// Reserved height for the live region.
const VIEWPORT_HEIGHT: u16 = 16;
const GAUGE_WIDTH: usize = 12;
const TICK: Duration = Duration::from_millis(50);

/// Inline-viewport TUI backend for `multi plan`.
pub(crate) struct PlanInlineTuiBackend {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    color: bool,
    frame: u64,
    torn_down: bool,
}

impl PlanInlineTuiBackend {
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
            frame: 0,
            torn_down: false,
        })
    }

    fn terminal_width(&self) -> u16 {
        self.terminal.size().map(|s| s.width).unwrap_or(80)
    }

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

    fn draw_live(&mut self, state: &PlanPresenterState) {
        let lines = plan_live_lines(state, self.frame, GAUGE_WIDTH, self.color);
        let _ = self.terminal.draw(|f| {
            let area = f.area();
            f.render_widget(Paragraph::new(lines), area);
        });
    }

    /// Flush the whole final record (every row's terminal state, the stale
    /// reason for every re-planned check, and the list of `.check-plan.toml`
    /// files written) into scrollback, once.
    fn flush_final_record(&mut self, state: &PlanPresenterState) {
        let width = self.terminal_width();
        let lines = plan_final_record_lines(state, self.color, width);
        if lines.is_empty() {
            return;
        }
        let height = lines.len() as u16;
        let _ = self.terminal.insert_before(height, move |buf| {
            let area = buf.area;
            Paragraph::new(lines).render(area, buf);
        });
    }
}

impl PlanRenderBackend for PlanInlineTuiBackend {
    fn apply(&mut self, _state: &PlanPresenterState, event: &PlanUiEvent) {
        if self.torn_down {
            return;
        }
        if let PlanUiEvent::Log(line) = event {
            self.flush_log_line(line);
        }
    }

    fn tick(&mut self, state: &PlanPresenterState) {
        if self.torn_down {
            return;
        }
        self.frame = self.frame.wrapping_add(1);
        self.draw_live(state);
    }

    fn teardown(&mut self, state: &PlanPresenterState) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        if state.rows.is_empty() {
            let _ = self.terminal.insert_before(1, |buf| {
                let area = buf.area;
                Paragraph::new(Line::raw("No requirements found.")).render(area, buf);
            });
        } else {
            self.flush_final_record(state);
        }
        let _ = self.terminal.clear();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, cursor::Show);
        let _ = stdout.flush();
    }

    fn tick_interval(&self) -> Duration {
        TICK
    }
}

/// The glyph for a row's current state: a spinner while active, a dim marker
/// for a reused plan, and a verdict mark once terminal.
fn leaf_glyph(state: &PlanRowState, frame: u64) -> &'static str {
    match state {
        PlanRowState::Queued => "·",
        PlanRowState::Verifying
        | PlanRowState::Stale
        | PlanRowState::Planning { .. }
        | PlanRowState::Retrying { .. }
        | PlanRowState::Calibrating => spinner_frame(frame),
        PlanRowState::Fresh => "○",
        PlanRowState::Planned { verdict: true } | PlanRowState::AgentOnly { verdict: true, .. } => {
            "✓"
        }
        PlanRowState::Planned { verdict: false }
        | PlanRowState::AgentOnly { verdict: false, .. } => "✗",
        PlanRowState::Error => "⚠",
    }
}

/// The reason tag for `row`, or `"?"` — defensively, in case a `Stale` event
/// was somehow never applied before a later one (unreachable through the real
/// orchestration, which always emits `Stale` before `Planning`/`Calibrating`).
fn stale_tag(row: &PlanRow) -> &'static str {
    row.stale_reason.map(StaleReason::tag).unwrap_or("?")
}

fn agent_only_tag(reason: AgentReason, verdict: bool) -> String {
    format!(
        "agent-only ({}, {})",
        agent_reason_str(reason),
        if verdict { "pass" } else { "fail" }
    )
}

/// The row's tag text — the ticket's per-state label, always plain text (never
/// color-only, so the no-color path reads identically minus styling).
fn row_tag(row: &PlanRow) -> String {
    match &row.state {
        PlanRowState::Queued => String::new(),
        PlanRowState::Verifying => "verifying".to_string(),
        PlanRowState::Fresh => {
            let base = "fresh".to_string();
            if row.truncated {
                format!("{base} · truncated")
            } else {
                base
            }
        }
        PlanRowState::Stale => format!("stale ({})", stale_tag(row)),
        PlanRowState::Planning { .. } => {
            let mut s = format!("planning · stale: {}", stale_tag(row));
            if let Some(progress) = &row.progress {
                s.push_str(&format!(" · turn {}/{}", progress.turn, progress.max_turns));
                if let Some(activity) = &progress.activity {
                    s.push_str(&format!(" · {activity}"));
                }
            }
            s
        }
        PlanRowState::Retrying { attempt } => {
            format!("retrying (attempt {attempt}) · stale: {}", stale_tag(row))
        }
        PlanRowState::Calibrating => format!("calibrating · stale: {}", stale_tag(row)),
        PlanRowState::Planned { verdict } => {
            let base = format!("planned (jev, {})", if *verdict { "pass" } else { "fail" });
            if row.truncated {
                format!("{base} · truncated")
            } else {
                base
            }
        }
        PlanRowState::AgentOnly { reason, verdict } => {
            let base = agent_only_tag(*reason, *verdict);
            if row.truncated {
                format!("{base} · truncated")
            } else {
                base
            }
        }
        PlanRowState::Error => format!(
            "error: {}",
            row.error_message.as_deref().unwrap_or("unknown")
        ),
    }
}

/// The gauge/footer header line: `plan {bar} done/total   fresh N ·
/// re-planned N · agent-only N · errors N · elapsed · model[ · N truncated]`.
/// The trailing truncated count, when present, is its own styled `Span` (a
/// warning color under `--enable-colors`) — but the text itself is unconditional,
/// so the no-color path still shows it plainly.
fn footer_line(state: &PlanPresenterState, gauge_width: usize, color: bool) -> Line<'static> {
    let done = state.done();
    let total = state.total.unwrap_or(0);
    let (fresh, replanned, agent_only, errors) = state.outcome_tallies();
    let bar = gauge_bar(done, total, gauge_width);
    let total_str = state
        .total
        .map(|t| t.to_string())
        .unwrap_or_else(|| "?".to_string());
    let elapsed = human_elapsed(state.run_started.elapsed());
    let base = format!(
        "plan  {bar} {done}/{total_str}   fresh {fresh} · re-planned {replanned} · agent-only {agent_only} · errors {errors} · {elapsed} · {}",
        state.model,
    );

    let truncated = state.truncated_count();
    if truncated == 0 {
        return Line::raw(base);
    }
    let suffix = format!(" · {truncated} truncated");
    if color {
        Line::from(vec![
            Span::raw(base),
            Span::styled(suffix, Style::new().fg(Color::Yellow)),
        ])
    } else {
        Line::raw(format!("{base}{suffix}"))
    }
}

/// Build the live region: the footer line, then the in-flight tree grouped by
/// requirement. Requirements with any active row sort first (liveness),
/// mirroring `crate::checks::presenter::inline::live_lines`.
fn plan_live_lines(
    state: &PlanPresenterState,
    frame: u64,
    gauge_width: usize,
    color: bool,
) -> Vec<Line<'static>> {
    let mut lines = vec![footer_line(state, gauge_width, color)];

    let mut reqs = state.requirement_indices();
    reqs.sort_by_key(|&r| {
        let active = state
            .requirement_rows(r)
            .iter()
            .any(|(_, row)| PlanPresenterState::is_active(&row.state));
        (!active, r)
    });

    for (i, &req_index) in reqs.iter().enumerate() {
        let rows = state.requirement_rows(req_index);
        let req_title = rows
            .first()
            .map(|(_, r)| r.req_title.clone())
            .unwrap_or_default();
        let settled = rows.iter().filter(|(_, r)| r.state.is_terminal()).count();
        let last_req = i + 1 == reqs.len();
        let parent_branch = if last_req { "└─ " } else { "├─ " };
        lines.push(Line::raw(format!(
            "{parent_branch}{req_title}  ({settled}/{})",
            rows.len()
        )));

        let cont = if last_req { "   " } else { "│  " };
        for (j, (_, row)) in rows.iter().enumerate() {
            let last_child = j + 1 == rows.len();
            let child_branch = if last_child { "└─ " } else { "├─ " };
            let glyph = leaf_glyph(&row.state, frame);
            let elapsed = row
                .started
                .map(|s| clock_elapsed(s.elapsed()))
                .unwrap_or_default();
            let tag = row_tag(row);
            lines.push(Line::raw(format!(
                "{cont}{child_branch}{glyph} {:<40}{elapsed:<8}{tag}",
                row.check_title,
            )));
        }
    }

    if !state.recent_logs.is_empty() {
        lines.push(Line::raw("logs"));
        for entry in &state.recent_logs {
            lines.push(Line::raw(format!("  {entry}")));
        }
    }

    lines
}

/// The permanent final record: every row grouped by requirement with its
/// terminal state, the stale reason for every re-planned check, a repeated
/// truncated-count summary, and the list of `.check-plan.toml` files written —
/// the ticket's exact contract for scrollback / non-TTY stdout.
fn plan_final_record_lines(
    state: &PlanPresenterState,
    color: bool,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for req_index in state.requirement_indices() {
        let rows = state.requirement_rows(req_index);
        let req_title = rows
            .first()
            .map(|(_, r)| r.req_title.clone())
            .unwrap_or_default();
        lines.extend(styled_wrapped_lines(
            &req_title,
            width,
            color.then(|| Style::new().add_modifier(Modifier::BOLD)),
        ));
        for (_, row) in rows {
            let mut text = format!("  {}: {}", row.check_title, row_tag(row));
            if matches!(
                row.state,
                PlanRowState::Planned { .. } | PlanRowState::AgentOnly { .. }
            ) && let Some(reason) = row.stale_reason
            {
                text.push_str(&format!(" (stale: {})", reason.tag()));
            }
            let style = color.then(|| match row.state {
                PlanRowState::Error => Style::new().fg(Color::Red),
                PlanRowState::Fresh => Style::new().add_modifier(Modifier::DIM),
                _ => Style::new(),
            });
            lines.extend(styled_wrapped_lines(&text, width, style));
        }
    }

    let truncated = state.truncated_count();
    if truncated > 0 {
        let total = state.rows.len();
        lines.extend(styled_wrapped_lines(
            &format!("{truncated} of {total} checks have truncated discovery"),
            width,
            color.then(|| Style::new().fg(Color::Yellow)),
        ));
    }

    if !state.plan_files_written.is_empty() {
        lines.push(Line::raw("Wrote plan files:"));
        for path in &state.plan_files_written {
            lines.extend(styled_wrapped_lines(
                &format!("  {}", path.display()),
                width,
                None,
            ));
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::jev::plan_file::AgentReason;

    fn queued(s: &mut PlanPresenterState, id: usize, req_index: usize) {
        s.apply(&PlanUiEvent::Queued {
            id,
            req_index,
            req_title: format!("Req {req_index}"),
            check_title: format!("Check {id}"),
        });
    }

    fn rendered(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn header_shows_the_model_and_no_truncated_suffix_when_zero() {
        let state = PlanPresenterState::new("jev-latest".into());
        let lines = plan_live_lines(&state, 0, GAUGE_WIDTH, false);
        let header = &rendered(&lines)[0];
        assert!(header.ends_with("jev-latest"), "{header}");
        assert!(!header.contains("truncated"), "{header}");
    }

    #[test]
    fn header_appends_the_truncated_count_as_plain_text() {
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0, 0);
        state.apply(&PlanUiEvent::Fresh {
            id: 0,
            truncated: true,
        });
        let lines = plan_live_lines(&state, 0, GAUGE_WIDTH, false);
        let header = &rendered(&lines)[0];
        assert!(header.contains("1 truncated"), "{header}");
    }

    /// MULTI-1829 acceptance: every row state renders a distinct, correct tag.
    #[test]
    fn every_row_state_renders_its_tag() {
        let cases: Vec<(PlanRowState, &str)> = vec![
            (PlanRowState::Queued, ""),
            (PlanRowState::Verifying, "verifying"),
            (PlanRowState::Fresh, "fresh"),
            (PlanRowState::Planning { attempt: 1 }, "planning · stale:"),
            (
                PlanRowState::Retrying { attempt: 1 },
                "retrying (attempt 1) · stale:",
            ),
            (PlanRowState::Calibrating, "calibrating · stale:"),
            (
                PlanRowState::Planned { verdict: true },
                "planned (jev, pass)",
            ),
            (
                PlanRowState::AgentOnly {
                    reason: AgentReason::JevDisagreed,
                    verdict: false,
                },
                "agent-only (jev disagreed, fail)",
            ),
            (PlanRowState::Error, "error:"),
        ];
        for (state, expect) in cases {
            let row = PlanRow {
                req_index: 0,
                req_title: "R".into(),
                check_title: "c".into(),
                state,
                stale_reason: Some(StaleReason::New),
                started: None,
                attempt: 0,
                progress: None,
                truncated: false,
                error_message: Some("boom".into()),
            };
            let tag = row_tag(&row);
            assert!(
                tag.contains(expect),
                "state {:?} rendered {tag:?}, expected to contain {expect:?}",
                row.state
            );
        }
    }

    /// MULTI-1829 acceptance: every stale reason renders its exact tag text.
    #[test]
    fn every_stale_reason_renders_its_exact_text() {
        let cases = [
            (StaleReason::New, "new"),
            (StaleReason::PromptChanged, "prompt changed"),
            (StaleReason::FilesChanged, "files changed"),
            (StaleReason::FileSetChanged, "file set changed"),
            (StaleReason::Forced, "forced"),
        ];
        for (reason, text) in cases {
            let row = PlanRow {
                req_index: 0,
                req_title: "R".into(),
                check_title: "c".into(),
                state: PlanRowState::Stale,
                stale_reason: Some(reason),
                started: None,
                attempt: 0,
                progress: None,
                truncated: false,
                error_message: None,
            };
            assert_eq!(row_tag(&row), format!("stale ({text})"));
        }
    }

    #[test]
    fn planning_row_shows_turn_and_activity_when_present() {
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0, 0);
        state.apply(&PlanUiEvent::Stale {
            id: 0,
            reason: StaleReason::FilesChanged,
        });
        state.apply(&PlanUiEvent::Planning { id: 0, attempt: 1 });
        state.apply(&PlanUiEvent::Progress {
            id: 0,
            attempt: 1,
            turn: 3,
            max_turns: 30,
            activity: Some("Read src/auth/sign.rs".into()),
        });
        let lines = plan_live_lines(&state, 0, GAUGE_WIDTH, false);
        let rows = rendered(&lines);
        assert!(
            rows.iter().any(|l| l.contains("stale: files changed")
                && l.contains("turn 3/30")
                && l.contains("Read src/auth/sign.rs")),
            "{rows:?}"
        );
    }

    #[test]
    fn a_fully_fresh_run_renders_every_row_fresh() {
        let mut state = PlanPresenterState::new("m".into());
        for id in 0..3 {
            queued(&mut state, id, 0);
            state.apply(&PlanUiEvent::Fresh {
                id,
                truncated: false,
            });
        }
        let lines = plan_live_lines(&state, 0, GAUGE_WIDTH, false);
        let rows = rendered(&lines);
        // Every row's own tag ends with the exact word "fresh" (never "fresh
        // N" — that shape only appears in the header's tally, which this
        // excludes by matching a trailing, space-preceded "fresh").
        let fresh_rows = rows.iter().filter(|l| l.ends_with("fresh")).count();
        assert_eq!(fresh_rows, 3, "{rows:?}");
        assert!(
            state.rows.values().all(|r| r.state == PlanRowState::Fresh),
            "every row settled Fresh"
        );
    }

    #[test]
    fn final_record_lists_stale_reasons_and_written_files() {
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0, 0);
        state.apply(&PlanUiEvent::Stale {
            id: 0,
            reason: StaleReason::PromptChanged,
        });
        state.apply(&PlanUiEvent::Planning { id: 0, attempt: 1 });
        state.apply(&PlanUiEvent::Calibrating { id: 0 });
        state.apply(&PlanUiEvent::Planned {
            id: 0,
            verdict: true,
            truncated: false,
        });
        state.apply(&PlanUiEvent::PlanFilesWritten(vec![
            std::path::PathBuf::from("services/keystore/.check-plan.toml"),
        ]));

        let lines = plan_final_record_lines(&state, false, 200);
        let rows = rendered(&lines);
        let joined = rows.join("\n");
        assert!(joined.contains("planned (jev, pass)"), "{joined}");
        assert!(joined.contains("stale: prompt changed"), "{joined}");
        assert!(joined.contains("Wrote plan files:"), "{joined}");
        assert!(
            joined.contains("services/keystore/.check-plan.toml"),
            "{joined}"
        );
    }

    #[test]
    fn final_record_repeats_the_truncated_count() {
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0, 0);
        state.apply(&PlanUiEvent::Fresh {
            id: 0,
            truncated: true,
        });
        let lines = plan_final_record_lines(&state, false, 200);
        let joined = rendered(&lines).join("\n");
        assert!(
            joined.contains("1 of 1 checks have truncated discovery"),
            "{joined}"
        );
    }
}
