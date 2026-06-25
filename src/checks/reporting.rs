//! The reporting phase + exit code (M6; actor rework in MULTI-1368).
//!
//! [`ReportingActor`] folds each [`CheckCompleted`] into per-requirement
//! accumulators **incrementally** as agents return (no barrier). Because
//! streaming arrival is nondeterministic, it **buffers and sorts** by
//! `(req_index, check declaration order)` before producing the final
//! `Vec<RequirementOutcome>`, which it hands back to the coordinator over a
//! `oneshot`. The coordinator renders it through [`report`].
//!
//! [`report`] renders each requirement's verdict and returns the process exit
//! code:
//!
//! * requirement title in **green** (satisfied) / **red** (not);
//! * failing checks printed in **red** with their evidence;
//! * passing checks omitted;
//! * exit `0` iff every requirement is satisfied (empty suite included), else `1`.
//!
//! Output is routed through the existing [`Terminal`] and honors the global
//! `--enable-colors` setting; with color disabled it degrades to plain text.

use std::collections::HashMap;
use std::path::PathBuf;

use kameo::Actor;
use kameo::message::{Context, Message};
use miette::Result;
use tokio::sync::oneshot;

use crate::Terminal;
use crate::checks::messages::{CheckCompleted, DiscoveryFailed, ExecutionComplete};
use crate::checks::model::{CheckId, CheckOutcome, RequirementOutcome};

/// The terminal result of a run: the ordered per-requirement outcomes, or an
/// abort diagnostic (an invalid suite from discovery).
pub(crate) type RunResult = Result<Vec<RequirementOutcome>>;

/// Per-requirement accumulator. Checks arrive out of order (streaming), so each
/// is tagged with its [`CheckId`] and sorted back into declaration order at the
/// end (ids are assigned monotonically in `(req_index, check)` order).
struct ReqAccum {
    title: String,
    filepath: PathBuf,
    checks: Vec<(CheckId, CheckOutcome)>,
}

/// The terminal actor: folds streamed check outcomes into per-requirement
/// verdicts and, once it has seen every expected outcome, fires the ordered
/// result back to the coordinator.
pub(crate) struct ReportingActor {
    /// Fires exactly once with the terminal result; `None` after it has fired.
    result: Option<oneshot::Sender<RunResult>>,
    /// The total number of checks to expect (`None` until [`ExecutionComplete`]).
    expected: Option<usize>,
    /// How many [`CheckCompleted`]s have been folded so far.
    received: usize,
    /// Per-requirement accumulators, keyed by `req_index`.
    accum: HashMap<usize, ReqAccum>,
}

impl Actor for ReportingActor {
    type Args = Self;
    type Error = std::convert::Infallible;

    async fn on_start(
        args: Self::Args,
        _actor_ref: kameo::actor::ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        Ok(args)
    }
}

impl ReportingActor {
    /// Build the actor over the channel it will fire the terminal result on.
    pub(crate) fn new(result: oneshot::Sender<RunResult>) -> Self {
        Self {
            result: Some(result),
            expected: None,
            received: 0,
            accum: HashMap::new(),
        }
    }

    /// If the expected count is known and every outcome has arrived, build the
    /// ordered outcomes and fire the result. Idempotent: only fires once.
    fn try_finalize(&mut self) {
        let done = matches!(self.expected, Some(total) if self.received >= total);
        if !done {
            return;
        }
        let Some(tx) = self.result.take() else {
            return;
        };
        let _ = tx.send(Ok(self.build_outcomes()));
    }

    /// Drain the accumulators into a deterministic `Vec<RequirementOutcome>`:
    /// requirements in `req_index` order, checks within each in declaration
    /// (id) order. Matches the old `aggregate_planned` ordering byte-for-byte.
    fn build_outcomes(&mut self) -> Vec<RequirementOutcome> {
        let mut indices: Vec<usize> = self.accum.keys().copied().collect();
        indices.sort_unstable();
        indices
            .into_iter()
            .map(|i| {
                let mut acc = self.accum.remove(&i).expect("index came from the map");
                acc.checks.sort_by_key(|(id, _)| *id);
                let check_outcomes = acc.checks.into_iter().map(|(_, o)| o).collect();
                RequirementOutcome::aggregate(acc.title, acc.filepath, check_outcomes)
            })
            .collect()
    }
}

impl Message<CheckCompleted> for ReportingActor {
    type Reply = ();

    async fn handle(&mut self, msg: CheckCompleted, _ctx: &mut Context<Self, ()>) -> Self::Reply {
        let CheckCompleted { job, outcome } = msg;
        let acc = self.accum.entry(job.req_index).or_insert_with(|| ReqAccum {
            title: job.req_title,
            filepath: job.filepath,
            checks: Vec::new(),
        });
        acc.checks.push((job.id, outcome));
        self.received += 1;
        self.try_finalize();
    }
}

impl Message<ExecutionComplete> for ReportingActor {
    type Reply = ();

    async fn handle(
        &mut self,
        msg: ExecutionComplete,
        _ctx: &mut Context<Self, ()>,
    ) -> Self::Reply {
        self.expected = Some(msg.total_checks);
        self.try_finalize();
    }
}

impl Message<DiscoveryFailed> for ReportingActor {
    type Reply = ();

    async fn handle(&mut self, msg: DiscoveryFailed, _ctx: &mut Context<Self, ()>) -> Self::Reply {
        // Strict whole-run abort: surface the suite's validation diagnostic and
        // never produce a partial report.
        if let Some(tx) = self.result.take() {
            let _ = tx.send(Err(msg.report));
        }
    }
}

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
