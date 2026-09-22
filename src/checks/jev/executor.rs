//! `JevExecutor` (MULTI-1825): the `CheckExecutor` `multi check`'s pipeline
//! runs under `--features jev` — decides a check from its frozen
//! `.check-plan.toml` entry wherever the plan has demonstrated it can be
//! trusted, and escalates to the reasoning agent (`inner`) everywhere else.
//! Injected via [`crate::checks::config::Resolved::build_executor`], the
//! single `cfg` switch between this and the default build's bare
//! [`crate::checks::executor::cersei::CerseiExecutor`].
//!
//! ## Decision table
//!
//! Evaluated once per check, at attempt 1 only (see [`Self::run_check`]'s
//! docs on retries), by [`Self::decide`]:
//!
//! | Plan state | Action | `decided_by` |
//! | -- | -- | -- |
//! | No manifest-derived root ([`RootSource::Manifest`]) | plans are unusable without a stable root ⇒ agent; no plan read or written; one `warn` per requirements file | `Agent` |
//! | No entry / `prompt_xxh64` mismatch / `decider = agent` | agent | `Agent` |
//! | `decider = jev` but the stored `jev.noul` no longer agrees with the stored verdict under the **configured** threshold | treat as agent-decided; computed from the plan alone, no model call | `Agent` |
//! | `Fresh` (or `--no-cache`, which forces this to `ReadsStale`) | stored verdict + evidence; zero Jev calls | `Cached` |
//! | `DiscoveryStale` | the relevant file set may have changed ⇒ agent | `Agent` |
//! | `ReadsStale`, live Jev `Satisfied` | satisfied — see the note below on what this means for a stored FAIL | `Jev` |
//! | `ReadsStale`, live Jev `Satisfied`, but the response `model` ≠ the plan's calibrated `jev.model` | calibration doesn't carry across model versions | `Agent` |
//! | `ReadsStale`, live Jev `NotVerified`/`OverBudget` | possible false positive ⇒ agent; its verdict is final | `Agent` |
//!
//! ### A stored FAIL under `ReadsStale`
//!
//! The live Jev call in the `ReadsStale` branch asks exactly one question —
//! "does the (fresh) evidence affirmatively demonstrate the check is
//! satisfied?" — independently of what the plan's `verdict` says. So this
//! table is followed **literally**, not "confirm the stored verdict":
//!
//! * A stored `verdict = false` (FAIL) whose fresh evidence now reads
//!   `Satisfied` settles as **satisfied**, `decided_by = Jev` — the frozen
//!   evidence changed (that's what `ReadsStale` means) and the live call
//!   affirmatively demonstrates the check now holds. Reusing the old
//!   `evidence` text here would be actively wrong (it explained the old
//!   failure), so the synthesized outcome carries a Jev-specific
//!   explanation instead (see [`jev_settled_outcome`]).
//! * A stored `verdict = true` (PASS) whose fresh evidence reads
//!   `NotVerified` does **not** settle as failed — it escalates to the
//!   agent, whose verdict is final (which may still come back PASS). A live
//!   `NotVerified` only means "this one call couldn't affirmatively
//!   confirm it," not "confirmed false": only a full agent run — the same
//!   authority a plan's `decider = agent` entry always deferred to — can
//!   report a genuine failure.
//!
//! ## Retries never redo Jev work
//!
//! The execution pipeline calls [`CheckExecutor::run_check`] once per
//! attempt. A `Cached`/`Jev` [`AgentOutcome`] always carries a verdict (see
//! that type's `decided_by` docs), so execution's retry loop — which only
//! retries a check whose agent finished *without* reporting — never retries
//! a `Cached`/`Jev` decision in practice. [`Self::run_check`] additionally
//! shortcuts to `inner` outright on `attempt > 1` regardless, so a plan
//! consult never even runs twice for the same check: the first attempt
//! either settled it or handed it to the agent, and only the agent path can
//! produce a second attempt.
//!
//! ## Errors
//!
//! A [`JevError`] from the live verify call maps to one of two shapes:
//! `Exhausted`/`Transport` (transient/single-request failures) escalate to
//! the agent — [`Decision::Agent`], not a failure. `Unauthorized`/
//! `MissingApiKey`/a non-context `Invalid` must **abort the whole run** — a
//! bad or missing credential must never silently read as every check
//! disagreeing with the agent — wrapped as [`AbortCheckRun`], which
//! `crate::checks::execution` detects via `Report::downcast_ref` and turns
//! into a whole-run abort (mirroring `multi plan`'s own `AbortPlanRun`
//! treatment of the identical error kinds).
//!
//! ## Self-healing (MULTI-1826, not this ticket)
//!
//! This ticket never writes `.check-plan.toml`. [`Self::decide`] is
//! deliberately the one, clearly separated decision step — it returns
//! [`Decision`], never touches the filesystem for writing, and every path
//! that escalates to the agent is a single call site
//! ([`Self::run_check`]'s `self.inner.run_check(req)`) — so MULTI-1826 can
//! wrap that one call site to also emit a `PlanUpdate` from the agent's
//! eventual outcome without restructuring this executor.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use miette::{Diagnostic, Report, Result};
use thiserror::Error;
use tokio::sync::OnceCell;

use crate::checks::config::JevConfig;
use crate::checks::executor::{AgentOutcome, AgentRunRequest, CheckExecutor, CheckReport};
use crate::checks::jev::client::JevClient;
use crate::checks::jev::error::JevError;
use crate::checks::jev::plan_file::{
    self, Decider, JevCalibration, PlanCheck, PlanFile, PlanStore,
};
use crate::checks::jev::replay::{self, Freshness, ReplayedCall};
use crate::checks::jev::verify::{self, Agreement, Evidence, Expected, JevDecision};
use crate::checks::model::{DecidedBy, RootSource};

/// A directory's lazily-loaded, at-most-once-parsed `.check-plan.toml` —
/// `None` for "no plan file" *or* "failed to load" (both decide `Agent`;
/// see [`JevExecutor::get_or_load_plan`]'s docs on why collapsing the two is
/// safe here). `Arc` so every check under the directory shares the one
/// parsed [`PlanFile`] rather than cloning it.
type PlanLoad = OnceCell<Option<Arc<PlanFile>>>;

/// A [`JevExecutor::run_check`] failure that must abort the *whole* `multi
/// check` run rather than merely error the one check it was raised for —
/// see the module docs' "Errors" section. `crate::checks::execution`
/// detects this via `miette::Report::downcast_ref` on a failed
/// `run_check`'s error and stops the run instead of retrying or reporting
/// this check as merely `Errored`.
#[derive(Debug, Error, Diagnostic)]
#[error("Jev rejected the check verification request: {0}")]
#[diagnostic(
    code(jev::check::abort),
    help("fix the underlying TypeSafe credential or request problem, then re-run `multi check`")
)]
pub(crate) struct AbortCheckRun(#[source] pub JevError);

/// [`JevExecutor::decide`]'s outcome — see the module docs' "Self-healing"
/// section on why escalating to the agent is its own variant rather than
/// `decide` running the agent itself.
enum Decision {
    /// Settle without running the agent: a fresh cache hit or a live Jev
    /// call that affirmatively demonstrated satisfaction. Carries the
    /// ready-to-return, well-formed [`AgentOutcome`].
    Settled(AgentOutcome),
    /// No usable plan decision (or the plan doesn't apply at all); the
    /// caller must run the agent.
    Agent,
}

/// Decides `multi check` from a frozen `.check-plan.toml` wherever it can,
/// escalating to `inner` (the reasoning agent) everywhere else — see the
/// module docs.
pub struct JevExecutor {
    inner: Arc<dyn CheckExecutor + Send + Sync>,
    jev_client: Arc<JevClient>,
    jev_config: JevConfig,
    /// `multi check --no-cache`: treat `Fresh` as `ReadsStale` so Jev is
    /// always consulted (never `Cached`) once a plan entry exists at all.
    /// Purely about **verdict** caching (the decision table's `Fresh` row) —
    /// orthogonal to [`Self::plan_cache`], the **in-memory parse** cache
    /// below, which always applies regardless of this flag.
    no_cache: bool,
    /// De-duplicates the "no manifest-derived root" warning to **one per
    /// requirements file**, not per check (the decision table) — keyed by
    /// [`crate::checks::executor::PlanIdentity::dir`]. A plain
    /// `Mutex<HashSet<..>>` rather than per-check-local state because
    /// concurrent checks under the same requirements file (the common case:
    /// several checks share one `CHECKS.md`) all reach this executor
    /// through the same `Arc`-shared instance, and the warning must
    /// de-duplicate across that concurrency, not just within one check.
    warned_no_root: Mutex<HashSet<PathBuf>>,
    /// Per-directory plan-load cache (code review on MULTI-1825): without
    /// this, every check under the same directory would independently
    /// `PlanStore::load` (re-read + re-parse) the identical
    /// `.check-plan.toml` — wasted work for a large suite's *good* plan,
    /// and a duplicate `warn` per check for a *corrupt* one. Two layers,
    /// deliberately: the outer `Mutex<HashMap<..>>` is locked only long
    /// enough to get-or-insert a directory's [`PlanLoad`] cell — never
    /// across the load itself, which is why this is a `std::sync::Mutex`
    /// (briefly held, never across an `.await`) rather than an async one.
    /// The inner [`OnceCell`] is what actually de-duplicates the load: its
    /// `get_or_init` guarantees the initializing future runs to completion
    /// **at most once** even when several checks race to consult the same
    /// directory concurrently — every racer awaits the *same* in-flight
    /// initialization rather than each starting (and, on a corrupt file,
    /// each warning about) its own.
    plan_cache: Mutex<HashMap<PathBuf, Arc<PlanLoad>>>,
}

impl JevExecutor {
    pub fn new(
        inner: Arc<dyn CheckExecutor + Send + Sync>,
        jev_client: Arc<JevClient>,
        jev_config: JevConfig,
        no_cache: bool,
    ) -> Self {
        Self {
            inner,
            jev_client,
            jev_config,
            no_cache,
            warned_no_root: Mutex::new(HashSet::new()),
            plan_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Load (and parse) the directory's `.check-plan.toml` at most once for
    /// this executor's whole lifetime — see [`Self::plan_cache`]'s docs.
    /// `None` covers both "no plan file exists" and "the plan failed to
    /// load" (a corrupt or unknown-version file): both decide `Agent`
    /// identically in [`Self::decide`], and collapsing them here is safe
    /// *because* the load failure is already warned about right here, the
    /// one place it can happen — a caller that only sees `None` has lost no
    /// information it would have acted on differently.
    async fn get_or_load_plan(&self, dir: &Path) -> Option<Arc<PlanFile>> {
        let cell = {
            let mut cache = self
                .plan_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache
                .entry(dir.to_path_buf())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        cell.get_or_init(|| async {
            match PlanStore::load(dir) {
                Ok(Some(plan)) => Some(Arc::new(plan)),
                Ok(None) => None,
                Err(err) => {
                    // See `decide`'s former docs on this exact message: a
                    // broken plan file degrades safely to the agent rather
                    // than aborting the whole run — but now warned at most
                    // once per directory, not once per check under it.
                    tracing::warn!(
                        dir = %dir.display(),
                        error = %err,
                        "failed to load .check-plan.toml; deciding checks under this directory with the agent",
                    );
                    None
                }
            }
        })
        .await
        .clone()
    }

    /// Consult the frozen plan for `req` and decide — see the module docs'
    /// decision table. `Err` only for an unrecoverable Jev failure that must
    /// abort the whole run (see the module docs' "Errors" section).
    async fn decide(&self, req: &AgentRunRequest) -> Result<Decision> {
        if req.plan.root_source != RootSource::Manifest {
            self.warn_once_no_root(&req.plan.dir);
            return Ok(Decision::Agent);
        }

        // Loaded (and, on a corrupt/unknown-version file, warned about) at
        // most once per directory for this executor's whole lifetime — see
        // `Self::plan_cache`'s and `get_or_load_plan`'s docs. Both "no plan
        // file at all" and "the plan failed to load" collapse to `None`
        // here (exactly "no usable entry" — a corrupt file degrades safely
        // to the agent rather than aborting the whole `multi check` run,
        // unlike a bad Jev credential, which would silently mis-decide
        // every remaining check the same way).
        let Some(plan) = self.get_or_load_plan(&req.plan.dir).await else {
            return Ok(Decision::Agent);
        };

        let prompt_hash = plan_file::prompt_xxh64(&req.check.title, &req.check.prompt);
        let Some(entry) = plan
            .lookup(
                &req.plan.source,
                req.plan.req_ordinal,
                req.plan.check_ordinal,
                &prompt_hash,
            )
            .cloned()
        else {
            // No entry, or `prompt_xxh64` no longer matches.
            return Ok(Decision::Agent);
        };

        let Decider::Jev = entry.decider else {
            return Ok(Decision::Agent);
        };

        let Some(calibration) = entry.jev.clone() else {
            // Defensive: every `decider = "jev"` entry `multi plan` ever
            // writes carries a calibration (see `plan::planner`'s
            // `decide_from_calibration`) — unreachable through a plan this
            // engine wrote itself, but a hand-edited file could violate it.
            // Fail safe to the agent rather than trusting an incomplete
            // entry.
            return Ok(Decision::Agent);
        };

        // "decider = jev but the stored calibration no longer agrees under
        // the CONFIGURED threshold (threshold was raised)" — computed
        // purely from the plan, no I/O and no model call, and checked
        // BEFORE replay: this is about whether the plan's own calibration
        // is still trustworthy under today's config, independent of
        // whether the underlying evidence has changed at all.
        let expected = if entry.verdict {
            Expected::Pass
        } else {
            Expected::Fail
        };
        if verify::agreement(expected, calibration.noul, self.jev_config.threshold)
            != Agreement::Agrees
        {
            return Ok(Decision::Agent);
        }

        let replayed = replay::replay_check(&req.check.title, &entry.calls, &req.source_dir).await;
        let freshness = if self.no_cache && replayed.freshness == Freshness::Fresh {
            Freshness::ReadsStale
        } else {
            replayed.freshness
        };

        match freshness {
            Freshness::Fresh => Ok(Decision::Settled(cached_outcome(&entry))),
            Freshness::DiscoveryStale => Ok(Decision::Agent),
            Freshness::ReadsStale => {
                self.decide_reads_stale(req, &calibration, &replayed.calls)
                    .await
            }
        }
    }

    /// The `ReadsStale` branch: ask Jev the live "is this satisfied"
    /// question over the replayed evidence and decide from its answer — see
    /// the module docs' decision table and its note on a stored FAIL.
    async fn decide_reads_stale(
        &self,
        req: &AgentRunRequest,
        calibration: &JevCalibration,
        replayed_calls: &[ReplayedCall],
    ) -> Result<Decision> {
        let evidence = build_evidence(replayed_calls);
        // Owned so the reference `verify::Requirement` borrows outlives the
        // `to_string_lossy()` temporary — mirrors `plan::planner`'s own
        // `plan_check` doing the same for the identical reason.
        let declared_in = req.declared_in.to_string_lossy().into_owned();
        let requirement = verify::Requirement {
            title: &req.plan.requirement_title,
            declared_in: &declared_in,
        };

        let decision = verify::verify(
            &self.jev_client,
            &self.jev_config,
            &requirement,
            &req.check,
            &evidence,
        )
        .await;

        match decision {
            Ok(JevDecision::Satisfied { noul, model, .. }) => {
                if model != calibration.model {
                    // Calibration doesn't carry across model versions (e.g.
                    // `jev-latest` moved) — the threshold was tuned against
                    // the plan's calibrated model, not this one.
                    return Ok(Decision::Agent);
                }
                Ok(Decision::Settled(jev_settled_outcome(noul, &model)))
            }
            // `NotVerified`: a possible false positive, not a confirmed
            // failure (see the module docs' note on a stored FAIL) — the
            // agent's verdict is final either way. `OverBudget`: the live
            // request itself couldn't fit; the agent decides instead.
            Ok(JevDecision::NotVerified { .. } | JevDecision::OverBudget) => Ok(Decision::Agent),
            Err(JevError::Exhausted { .. } | JevError::Transport(_)) => Ok(Decision::Agent),
            Err(
                err @ (JevError::Unauthorized | JevError::MissingApiKey | JevError::Invalid { .. }),
            ) => Err(Report::new(AbortCheckRun(err))),
        }
    }

    /// Warn at most once per requirements file (keyed by `dir`) that its
    /// root isn't manifest-derived, so a plan can't be consulted for any of
    /// its checks — see [`Self::warned_no_root`]'s docs.
    fn warn_once_no_root(&self, dir: &Path) {
        let mut warned = self
            .warned_no_root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if warned.insert(dir.to_path_buf()) {
            tracing::warn!(
                dir = %dir.display(),
                "no MultiTool.toml manifest found above this requirements file; \
                 its plan (if any) is unusable — every check under it runs the agent",
            );
        }
    }
}

#[async_trait]
impl CheckExecutor for JevExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        // Retries never redo Jev work — see the module docs.
        if req.attempt > 1 {
            return self.inner.run_check(req).await;
        }

        match self.decide(&req).await? {
            Decision::Settled(outcome) => Ok(outcome),
            Decision::Agent => self.inner.run_check(req).await,
        }
    }
}

/// Build a `Fresh`-settled [`AgentOutcome`] straight from the plan's stored
/// verdict + evidence: zero model calls, zero agent turns, no trace, no
/// captured tool calls (the plan's evidence was replayed, not gathered by a
/// live agent). See the module docs' decision table's `Fresh` row.
fn cached_outcome(entry: &PlanCheck) -> AgentOutcome {
    AgentOutcome {
        verdict: Some(CheckReport {
            success: entry.verdict,
            evidence: entry.evidence.clone(),
        }),
        stop_reason: Some("settled from a fresh cached plan entry".to_string()),
        turns: 0,
        error: None,
        trace_jsonl: None,
        tool_calls: Vec::new(),
        decided_by: DecidedBy::Cached,
    }
}

/// Build a `Jev`-settled, satisfied [`AgentOutcome`] from a live Jev call —
/// see the module docs' note on a stored FAIL for why this is always
/// `success: true` (only [`JevDecision::Satisfied`] ever reaches this) and
/// why it never reuses the plan's stored `evidence` text.
fn jev_settled_outcome(noul: f64, model: &str) -> AgentOutcome {
    AgentOutcome {
        verdict: Some(CheckReport {
            success: true,
            evidence: Some(format!(
                "Jev verified the replayed evidence affirmatively demonstrates this check is satisfied (noul {noul:.3}, model {model})"
            )),
        }),
        stop_reason: Some("settled by Jev".to_string()),
        turns: 0,
        error: None,
        trace_jsonl: None,
        tool_calls: Vec::new(),
        decided_by: DecidedBy::Jev,
    }
}

/// Build the `state.evidence` Jev sees from replayed calls: excludes any
/// [`ReplayedCall::missing`] or [`ReplayedCall::truncated`] call (neither
/// has a reproducible, trustworthy result to hand Jev), and fills
/// [`Evidence::windowed`] straight from [`ReplayedCall::windowed`] — exactly
/// as `crate::checks::jev::verify`'s own module docs describe for a future
/// `multi check` caller. Reachable only from the `ReadsStale` branch, whose
/// aggregation guarantees at least one call survives this filter (a
/// `missing` or truncation-mismatched call would itself force
/// `DiscoveryStale`, not `ReadsStale` — see `crate::checks::jev::replay`'s
/// `classify_call` docs).
fn build_evidence(calls: &[ReplayedCall]) -> Vec<Evidence> {
    calls
        .iter()
        .filter(|c| !c.missing && !c.truncated)
        .filter_map(|c| {
            Some(Evidence {
                tool: c.tool,
                input: c.input.clone(),
                output: c.output.clone()?,
                windowed: c.windowed,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
