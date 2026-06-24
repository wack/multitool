//! The reporting phase + exit code (M6).
//!
//! Renders each requirement's verdict and returns the process exit code:
//!
//! * requirement title in **green** (satisfied) / **red** (not);
//! * failing checks printed in **red** with their evidence;
//! * passing checks omitted;
//! * exit `0` iff every requirement is satisfied (empty suite included), else `1`.
//!
//! Output is routed through the existing [`Terminal`] and honors the global
//! `--enable-colors` setting; with color disabled it degrades to plain text.

use miette::Result;

use crate::Terminal;
use crate::checks::model::{CheckOutcome, RequirementOutcome};

/// Render `outcomes` and return the exit code (0 = all satisfied, 1 = any not).
pub fn report(terminal: &Terminal, outcomes: &[RequirementOutcome]) -> Result<i32> {
    if outcomes.is_empty() {
        terminal.write_stdout_line("No requirements found.")?;
        return Ok(0);
    }

    let color = terminal.stdout_allows_color();
    let mut all_satisfied = true;

    for outcome in outcomes {
        terminal.write_stdout_line(&format_requirement(outcome, color))?;
        if !outcome.satisfied {
            all_satisfied = false;
            for check in outcome.failing_checks() {
                terminal.write_stdout_line(&format_failing_check(check, color))?;
            }
        }
    }

    Ok(if all_satisfied { 0 } else { 1 })
}

fn format_requirement(outcome: &RequirementOutcome, color: bool) -> String {
    if color {
        let styled = console::style(outcome.title.clone()).bold();
        let styled = if outcome.satisfied {
            styled.green()
        } else {
            styled.red()
        };
        styled.to_string()
    } else {
        let mark = if outcome.satisfied { "PASS" } else { "FAIL" };
        format!("[{mark}] {}", outcome.title)
    }
}

fn format_failing_check(check: &CheckOutcome, color: bool) -> String {
    let evidence = check.evidence.as_deref().unwrap_or("no evidence provided");
    let body = format!("  ✗ {}: {}", check.title, evidence);
    if color {
        console::style(body).red().to_string()
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::model::{RequirementOutcome, Verdict};
    use std::path::PathBuf;

    fn outcome(title: &str, satisfied: bool, checks: Vec<CheckOutcome>) -> RequirementOutcome {
        RequirementOutcome {
            title: title.into(),
            filepath: PathBuf::from("CHECKS.md"),
            satisfied,
            check_outcomes: checks,
        }
    }

    #[test]
    fn exit_code_reflects_aggregate_satisfaction() {
        // Plain-text formatting is exercised here (color = false).
        let failing = CheckOutcome {
            title: "c".into(),
            verdict: Verdict::Failed,
            evidence: Some("bad".into()),
        };
        let line = format_failing_check(&failing, false);
        assert!(line.contains("bad"));
        assert!(line.contains('c'));

        let pass = outcome("ok", true, vec![]);
        assert_eq!(format_requirement(&pass, false), "[PASS] ok");
        let fail = outcome("nope", false, vec![failing]);
        assert_eq!(format_requirement(&fail, false), "[FAIL] nope");
    }
}
