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

use super::state::{PlanPresenterState, PlanRow, PlanRowState, StaleReason};
use super::{FinalCheck, FinalOutcome, FinalRecord, PlanRenderBackend, PlanUiEvent};

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
    /// files written) into scrollback, once — rendered from `record`, the
    /// run's one authoritative value (MULTI-1829 code review), never from
    /// live [`PlanPresenterState`], which may have missed best-effort events.
    fn flush_final_record(&mut self, record: &FinalRecord) {
        let width = self.terminal_width();
        let lines = final_record_lines(record, self.color, width);
        if lines.is_empty() {
            return;
        }
        let height = lines.len() as u16;
        let _ = self.terminal.insert_before(height, move |buf| {
            let area = buf.area;
            Paragraph::new(lines).render(area, buf);
        });
    }

    /// Degraded fallback for when no [`FinalRecord`] was ever built (the run
    /// aborted before `plan::run_with_planner` reached that point, e.g. an
    /// `AbortPlanRun` credential failure) — flush whatever the live view
    /// captured from best-effort events instead of nothing. No written-files
    /// list: that data lives only in a `FinalRecord`, which this run never
    /// produced.
    fn flush_partial_state(&mut self, state: &PlanPresenterState) {
        let width = self.terminal_width();
        let lines = partial_record_lines(state, self.color, width);
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

    fn teardown(&mut self, state: &PlanPresenterState, final_record: Option<&FinalRecord>) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        match final_record {
            // The normal path: a complete, authoritative record was built.
            // `total_checks` is always ≥ 1 here (a `FinalRecord` is only
            // ever built past `run_with_planner`'s early "no requirements"
            // return, and every requirement has ≥ 1 check by construction),
            // so this is never the "No requirements found." case.
            Some(record) => self.flush_final_record(record),
            // No requirements were ever discovered — the presenter never
            // received a single row either.
            None if state.rows.is_empty() => {
                let _ = self.terminal.insert_before(1, |buf| {
                    let area = buf.area;
                    Paragraph::new(Line::raw("No requirements found.")).render(area, buf);
                });
            }
            // The run aborted before a `FinalRecord` was ever built — fall
            // back to the live view's own best-effort state.
            None => self.flush_partial_state(state),
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

/// One [`FinalCheck`]'s tag text — mirrors [`row_tag`]'s terminal-state arms
/// exactly (same text conventions), just built from the authoritative
/// [`FinalRecord`] rather than a live [`PlanRow`].
fn final_check_tag(check: &FinalCheck) -> String {
    let base = match &check.outcome {
        FinalOutcome::Fresh => "fresh".to_string(),
        FinalOutcome::Planned { verdict } => {
            format!("planned (jev, {})", if *verdict { "pass" } else { "fail" })
        }
        FinalOutcome::AgentOnly { reason, verdict } => agent_only_tag(*reason, *verdict),
        FinalOutcome::Error { message } => format!("error: {message}"),
    };
    if check.truncated {
        format!("{base} · truncated")
    } else {
        base
    }
}

/// The permanent final record: every requirement's checks with their
/// terminal state, the stale reason for every re-planned check, a repeated
/// truncated-count summary, and the list of `.check-plan.toml` files written —
/// the ticket's exact contract for scrollback / non-TTY stdout. Built from
/// `record` alone (MULTI-1829 code review: never from best-effort live
/// state), so it is always complete and correct once a [`FinalRecord`] exists
/// at all.
fn final_record_lines(record: &FinalRecord, color: bool, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    for req in &record.requirements {
        lines.extend(styled_wrapped_lines(
            &req.title,
            width,
            color.then(|| Style::new().add_modifier(Modifier::BOLD)),
        ));
        for check in &req.checks {
            let mut text = format!("  {}: {}", check.title, final_check_tag(check));
            if let Some(reason) = check.stale_reason {
                text.push_str(&format!(" (stale: {})", reason.tag()));
            }
            let style = color.then(|| match &check.outcome {
                FinalOutcome::Error { .. } => Style::new().fg(Color::Red),
                FinalOutcome::Fresh => Style::new().add_modifier(Modifier::DIM),
                _ => Style::new(),
            });
            lines.extend(styled_wrapped_lines(&text, width, style));
        }
    }

    if record.truncated_count > 0 {
        lines.extend(styled_wrapped_lines(
            &format!(
                "{} of {} checks have truncated discovery",
                record.truncated_count, record.total_checks
            ),
            width,
            color.then(|| Style::new().fg(Color::Yellow)),
        ));
    }

    if !record.written_paths.is_empty() {
        lines.push(Line::raw("Wrote plan files:"));
        for path in &record.written_paths {
            lines.extend(styled_wrapped_lines(
                &format!("  {}", path.display()),
                width,
                None,
            ));
        }
    }

    lines
}

/// Degraded fallback rendered straight from live [`PlanPresenterState`] — see
/// [`PlanInlineTuiBackend::flush_partial_state`]'s docs for when this is
/// used. No written-files section: that data only ever lives in a
/// [`FinalRecord`], which this path exists precisely because one was never
/// built.
fn partial_record_lines(state: &PlanPresenterState, color: bool, width: u16) -> Vec<Line<'static>> {
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

    use super::super::FinalRequirement;

    fn record_check(
        title: &str,
        outcome: FinalOutcome,
        stale_reason: Option<StaleReason>,
    ) -> FinalCheck {
        FinalCheck {
            title: title.into(),
            outcome,
            stale_reason,
            truncated: false,
        }
    }

    /// MULTI-1829 code review (blocking item 4): the inline backend's final
    /// record renders from a [`FinalRecord`] value, not live state — this
    /// exercises exactly that path (never `PlanPresenterState`/`PlanUiEvent`).
    #[test]
    fn final_record_lists_stale_reasons_and_written_files() {
        let record = FinalRecord {
            requirements: vec![FinalRequirement {
                title: "Keystore".into(),
                checks: vec![record_check(
                    "Sign",
                    FinalOutcome::Planned { verdict: true },
                    Some(StaleReason::PromptChanged),
                )],
            }],
            truncated_count: 0,
            total_checks: 1,
            written_paths: vec![std::path::PathBuf::from(
                "services/keystore/.check-plan.toml",
            )],
        };

        let lines = final_record_lines(&record, false, 200);
        let joined = rendered(&lines).join("\n");
        assert!(joined.contains("planned (jev, pass)"), "{joined}");
        assert!(joined.contains("stale: prompt changed"), "{joined}");
        assert!(joined.contains("Wrote plan files:"), "{joined}");
        assert!(
            joined.contains("services/keystore/.check-plan.toml"),
            "{joined}"
        );
    }

    /// MULTI-1829 code review (blocking item 3): the final record shows the
    /// stale reason for every re-planned check — every one of the ticket's
    /// five reasons, for both `Planned` and `AgentOnly` outcomes.
    #[test]
    fn final_record_shows_every_stale_reason_for_planned_and_agent_only() {
        let reasons = [
            StaleReason::New,
            StaleReason::PromptChanged,
            StaleReason::FilesChanged,
            StaleReason::FileSetChanged,
            StaleReason::Forced,
        ];
        let mut checks = Vec::new();
        for (i, reason) in reasons.iter().enumerate() {
            checks.push(record_check(
                &format!("planned-{i}"),
                FinalOutcome::Planned { verdict: true },
                Some(*reason),
            ));
            checks.push(record_check(
                &format!("agent-only-{i}"),
                FinalOutcome::AgentOnly {
                    reason: AgentReason::JevUncertain,
                    verdict: false,
                },
                Some(*reason),
            ));
        }
        let record = FinalRecord {
            requirements: vec![FinalRequirement {
                title: "R".into(),
                checks,
            }],
            truncated_count: 0,
            total_checks: reasons.len() * 2,
            written_paths: vec![],
        };

        let joined = rendered(&final_record_lines(&record, false, 200)).join("\n");
        for reason in reasons {
            let text = format!("(stale: {})", reason.tag());
            assert_eq!(
                joined.matches(&text).count(),
                2,
                "expected {text:?} for both Planned and AgentOnly in:\n{joined}"
            );
        }
    }

    /// Fresh and Error outcomes never carry a stale reason in the final
    /// record — only a re-planned check (`Planned`/`AgentOnly`) does.
    #[test]
    fn final_record_never_shows_a_stale_reason_for_fresh_or_error() {
        let record = FinalRecord {
            requirements: vec![FinalRequirement {
                title: "R".into(),
                checks: vec![
                    record_check("a", FinalOutcome::Fresh, None),
                    record_check(
                        "b",
                        FinalOutcome::Error {
                            message: "boom".into(),
                        },
                        None,
                    ),
                ],
            }],
            truncated_count: 0,
            total_checks: 2,
            written_paths: vec![],
        };
        let joined = rendered(&final_record_lines(&record, false, 200)).join("\n");
        assert!(!joined.contains("stale:"), "{joined}");
    }

    #[test]
    fn final_record_repeats_the_truncated_count() {
        let record = FinalRecord {
            requirements: vec![FinalRequirement {
                title: "R".into(),
                checks: vec![FinalCheck {
                    title: "a".into(),
                    outcome: FinalOutcome::Fresh,
                    stale_reason: None,
                    truncated: true,
                }],
            }],
            truncated_count: 1,
            total_checks: 1,
            written_paths: vec![],
        };
        let joined = rendered(&final_record_lines(&record, false, 200)).join("\n");
        assert!(
            joined.contains("1 of 1 checks have truncated discovery"),
            "{joined}"
        );
    }

    /// MULTI-1829 code review (blocking item 4): when no `FinalRecord` was
    /// ever built (the abort path), the inline backend still flushes
    /// *something* useful from the live view rather than nothing.
    #[test]
    fn partial_record_falls_back_to_live_state_when_no_final_record_exists() {
        let mut state = PlanPresenterState::new("m".into());
        queued(&mut state, 0, 0);
        state.apply(&PlanUiEvent::Stale {
            id: 0,
            reason: StaleReason::New,
        });
        state.apply(&PlanUiEvent::Planning { id: 0, attempt: 1 });
        state.apply(&PlanUiEvent::AgentOnly {
            id: 0,
            reason: AgentReason::ControlFailed,
            verdict: false,
            truncated: false,
        });
        let joined = rendered(&partial_record_lines(&state, false, 200)).join("\n");
        assert!(
            joined.contains("agent-only (control failed, fail)"),
            "{joined}"
        );
        assert!(joined.contains("stale: new"), "{joined}");
    }
}
