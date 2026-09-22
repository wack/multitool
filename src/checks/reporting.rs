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
#[cfg(feature = "jev")]
use crate::checks::messages::AbortRun;
use crate::checks::messages::{CheckCompleted, DiscoveryFailed, ExecutionComplete};
use crate::checks::model::{
    CheckId, CheckOutcome, DecidedBy, RequirementOutcome, decided_by_summary,
};

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

/// MULTI-1825, `--features jev` only: mirrors [`Message<DiscoveryFailed>`]
/// exactly — a check's executor hit an unrecoverable Jev failure, so the
/// whole run aborts with that diagnostic rather than producing a partial
/// report. `try_finalize`'s idempotence (`self.result.take()`) means
/// whichever of an `AbortRun` or a later, ordinary finalization arrives
/// first wins; once this fires, every subsequent `CheckCompleted` this
/// actor still receives from in-flight checks is folded into `accum` and
/// simply never read.
#[cfg(feature = "jev")]
impl Message<AbortRun> for ReportingActor {
    type Reply = ();

    async fn handle(&mut self, msg: AbortRun, _ctx: &mut Context<Self, ()>) -> Self::Reply {
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

    // MULTI-1827: a one-line decider-count summary after the requirement
    // list, but only when some check wasn't agent-decided — never true in
    // the default build (every check is `DecidedBy::Agent`), so this line
    // never appears in default-feature output.
    if let Some(summary) = decided_by_summary_line(outcomes) {
        terminal.write_stdout_line(&summary)?;
    }

    Ok(if all_satisfied { 0 } else { 1 })
}

/// Tally every check's decider across `outcomes`: `(cached, jev, agent)`.
fn decided_by_tallies(outcomes: &[RequirementOutcome]) -> (usize, usize, usize) {
    let mut cached = 0;
    let mut jev = 0;
    let mut agent = 0;
    for outcome in outcomes {
        for check in &outcome.check_outcomes {
            match check.decided_by {
                DecidedBy::Cached => cached += 1,
                DecidedBy::Jev => jev += 1,
                DecidedBy::Agent => agent += 1,
            }
        }
    }
    (cached, jev, agent)
}

/// The `report` summary line built from `outcomes`' deciders, or `None` when
/// every check was agent-decided (see [`decided_by_summary`]).
fn decided_by_summary_line(outcomes: &[RequirementOutcome]) -> Option<String> {
    let (cached, jev, agent) = decided_by_tallies(outcomes);
    decided_by_summary(cached, jev, agent)
}

/// The process exit code for a set of outcomes, without writing anything: `0` if
/// every requirement is satisfied (empty suite included), else `1`. Used by the
/// TTY path, where the presenter has already written the record to scrollback so
/// [`report`] must not also write to stdout.
pub(crate) fn exit_code(outcomes: &[RequirementOutcome]) -> i32 {
    if outcomes.iter().all(|o| o.satisfied) {
        0
    } else {
        1
    }
}

/// The plain-text requirement line (`[PASS]`/`[FAIL] title`). Exposed so the
/// inline presenter can assert its flushed record matches this exactly.
pub(crate) fn format_requirement_plain(outcome: &RequirementOutcome) -> String {
    format_requirement(outcome, false)
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

/// The plain-text failing-check line (`  ✗ title: evidence`, or
/// `  ✗ title: evidence [cached]`/`[jev]` when the check wasn't agent-decided
/// — MULTI-1827). Shared with the inline presenter so the TTY scrollback
/// record and the non-TTY stdout report render a failing check identically.
pub(crate) fn failing_check_text(check: &CheckOutcome) -> String {
    let evidence = check.evidence.as_deref().unwrap_or("no evidence provided");
    let mut text = format!("  ✗ {}: {}", check.title, evidence);
    if let Some(tag) = check.decided_by.tag() {
        text.push_str(&format!(" [{tag}]"));
    }
    text
}

fn format_failing_check(check: &CheckOutcome, color: bool) -> String {
    let body = failing_check_text(check);
    if color {
        console::style(body).red().to_string()
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::model::{DecidedBy, RequirementOutcome, Verdict};
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
            decided_by: DecidedBy::Agent,
        };
        let line = format_failing_check(&failing, false);
        assert!(line.contains("bad"));
        assert!(line.contains('c'));

        let pass = outcome("ok", true, vec![]);
        assert_eq!(format_requirement(&pass, false), "[PASS] ok");
        let fail = outcome("nope", false, vec![failing]);
        assert_eq!(format_requirement(&fail, false), "[FAIL] nope");
    }

    fn check(decided_by: DecidedBy, satisfied: bool) -> CheckOutcome {
        CheckOutcome {
            title: "c".into(),
            verdict: if satisfied {
                Verdict::Satisfied
            } else {
                Verdict::Failed
            },
            evidence: Some("evidence".into()),
            decided_by,
        }
    }

    /// MULTI-1827 acceptance: a failing check shows its decider next to the
    /// evidence — `cached`/`jev` — while an agent-decided failure (the only
    /// kind in the default build) renders exactly as before, no tag at all.
    #[test]
    fn failing_check_shows_its_decider_next_to_the_evidence() {
        let agent = failing_check_text(&check(DecidedBy::Agent, false));
        assert_eq!(agent, "  ✗ c: evidence");

        let cached = failing_check_text(&check(DecidedBy::Cached, false));
        assert_eq!(cached, "  ✗ c: evidence [cached]");

        let jev = failing_check_text(&check(DecidedBy::Jev, false));
        assert_eq!(jev, "  ✗ c: evidence [jev]");
    }

    /// MULTI-1827 acceptance: the one-line decider-count summary appears
    /// only once some check in the run was not agent-decided.
    #[test]
    fn summary_line_appears_only_when_a_check_was_not_agent_decided() {
        let all_agent = vec![outcome(
            "r",
            true,
            vec![check(DecidedBy::Agent, true), check(DecidedBy::Agent, true)],
        )];
        assert_eq!(decided_by_summary_line(&all_agent), None);

        let mixed = vec![outcome(
            "r",
            false,
            vec![
                check(DecidedBy::Agent, true),
                check(DecidedBy::Cached, true),
                check(DecidedBy::Jev, false),
            ],
        )];
        assert_eq!(
            decided_by_summary_line(&mixed),
            Some("1 cached · 1 jev · 1 agent".to_string())
        );
    }

    /// MULTI-1827 acceptance: an all-`Agent` run — every check in the
    /// default build — renders BYTE-FOR-BYTE what it rendered before this
    /// ticket. The expected strings were captured running this exact
    /// scenario (a satisfied requirement "R1"/check "c1", and a failing
    /// requirement "R2"/check "c2" with evidence "nope") against
    /// `format_requirement`/`failing_check_text` on the parent commit,
    /// `robbie/multi-1826` (pre-MULTI-1827). A prior version of this test
    /// only asserted the absence of `cached`/`jev` substrings, which would
    /// still pass if a line picked up a stray character or spacing drift;
    /// full string equality closes that hole.
    #[test]
    fn all_agent_run_renders_identically_to_before() {
        let c1 = CheckOutcome {
            title: "c1".into(),
            verdict: Verdict::Satisfied,
            evidence: None,
            decided_by: DecidedBy::Agent,
        };
        let c2 = CheckOutcome {
            title: "c2".into(),
            verdict: Verdict::Failed,
            evidence: Some("nope".into()),
            decided_by: DecidedBy::Agent,
        };
        let r1 = outcome("R1", true, vec![c1]);
        let r2 = outcome("R2", false, vec![c2.clone()]);

        assert_eq!(format_requirement(&r1, false), "[PASS] R1");
        assert_eq!(format_requirement(&r2, false), "[FAIL] R2");
        assert_eq!(failing_check_text(&c2), "  ✗ c2: nope");

        assert_eq!(decided_by_summary_line(&[r1, r2]), None);
    }
}
