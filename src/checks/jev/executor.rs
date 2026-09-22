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
//! ## Self-healing (MULTI-1826)
//!
//! Entry-building is **off the verdict path**, and deferred entirely until
//! every check in the run has settled — not merely "after the returning
//! call," which turned out not to be a strong enough barrier (code review):
//! an earlier version of this module `tokio::spawn`ed the expensive
//! replay/calibration work from inside [`Self::run_agent`] immediately after
//! capturing the outcome, reasoning that spawning happens after the value is
//! already in hand. On a multi-threaded runtime that's not an ordering
//! guarantee — the spawned task can be picked up by another worker thread
//! and start making Jev calls before the *spawning* call itself has
//! returned its result up through `run_check` to the caller that reports the
//! check as settled. So [`Self::run_agent`] and
//! [`Self::decide_reads_stale`]'s `Satisfied` arm only ever *record* queued
//! work on [`healer::Healer`] — [`healer::Healer::queue_agent_escalation`]/
//! [`healer::Healer::queue_refresh`] are both cheap, purely synchronous, and
//! never spawn anything (see that module's docs). The actual replay/Jev
//! calls only start inside [`healer::Healer::finish`], called from
//! [`Self::finalize`] (the [`CheckExecutor::finalize`] override), which
//! `crate::checks::run` calls exactly once, strictly after
//! `crate::checks::run_pipeline` has returned successfully — i.e. after
//! *every* check in the run has already been settled and reported. This
//! makes "the check's outcome is emitted before any calibration call is
//! made" true by construction, not by timing.
//!
//! An aborted run (a whole-run [`AbortCheckRun`], `--features jev` only)
//! skips [`Self::finalize`] entirely — see `crate::checks::run`'s call site
//! — so an abort can queue no further Jev calls and writes nothing, even
//! though some checks may have already queued healing work before the abort
//! fired.
//!
//! [`healer::Healer::finish`] bounds how many entry-building tasks
//! (`crate::checks::plan::entry_from_outcome` — replay plus up to two Jev
//! calls) run concurrently, and time-boxes each one individually, so neither
//! a large batch of queued escalations nor one stuck replay/request can hold
//! `finalize` — and therefore the process's exit — open indefinitely (code
//! review; see `healer`'s `MAX_CONCURRENT_HEALS`/`HEAL_TIMEOUT`). Every
//! update collected — both kinds — is then applied to its directory's plan
//! and written **once**. A failure building or writing one check's entry
//! (including a timeout, or the building task itself panicking) is logged
//! and that check's previous entry is left exactly as it was — self-healing
//! can never change a verdict or the run's exit code, both already final by
//! the time any of this runs.
//!
//! `--frozen` (jev builds only, absent from the default-feature build's clap
//! surface — see `crate::config::CheckSubcommand::frozen`) disables
//! self-healing entirely: [`Self::run_agent`] and
//! [`Self::decide_reads_stale`] never queue anything, and [`Self::finalize`]
//! writes nothing, so a frozen run never touches a `.check-plan.toml` and
//! never spends an extra Jev call on it. It does **not** disable the
//! `ReadsStale` decision table row's own live Jev consult — that call
//! decides *this run's* verdict, not a plan write.
//!
//! ### Recovering the sandbox root after the agent has already run
//!
//! [`entry_from_outcome`](crate::checks::plan::entry_from_outcome) needs the
//! sandbox root a captured call is relative to, but
//! [`AgentRunRequest`] is moved into `inner.run_check`, and by the time it
//! returns, the sandbox (and the request that owned its lease) may already
//! be torn down. Three fixes were considered:
//!
//! 1. Have the inner executor record the sandbox path it acquired onto the
//!    outcome it returns — a small, unconditional field
//!    ([`AgentOutcome::sandbox_root`]).
//! 2. Acquire the lease in `JevExecutor` itself, before delegating.
//! 3. Relativize the captured calls *inside* the inner executor, before it
//!    returns.
//!
//! (1) is what this ticket does. (2) changes sandbox-acquisition timing
//! (today only the agent-running executor ever acquires one — MULTI-1818),
//! which is out of this ticket's scope to disturb, and would still leave the
//! *default build's* [`CerseiExecutor`](crate::checks::executor::cersei::CerseiExecutor)
//! acquiring a lease it currently owns and controls the lifetime of. (3)
//! would leak `.check-plan.toml`-shaped concerns (relativizing against a
//! sandbox root, `PlanError`) into the executor that has nothing to do with
//! plans in the default build. (1) is the smallest, most local change:
//! `CerseiExecutor` (and the test [`FakeExecutor`](crate::checks::executor::FakeExecutor))
//! already know their own working directory, so recording it onto the
//! outcome costs nothing and keeps every plan-specific concern inside the
//! `jev` feature.

mod healer;

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
    self, Decider, JevCalibration, PlanCall, PlanCheck, PlanFile, PlanRequirement, PlanStore,
};
use crate::checks::jev::replay::{self, Freshness, ReplayedCall};
use crate::checks::jev::verify::{self, Agreement, Evidence, Expected, JevDecision};
use crate::checks::model::{DecidedBy, RootSource};
use healer::{HealContext, Healer, Update};

/// A directory's plan-loading result, lazily computed at most once for this
/// executor's whole lifetime — see [`JevExecutor::plan_cache`]'s docs.
/// [`Self::Missing`]/[`Self::Corrupt`] both decide `Agent` identically in
/// [`JevExecutor::decide`] (see [`JevExecutor::get_or_load_plan`]), but
/// self-healing (MULTI-1826) needs to tell them apart: [`JevExecutor::finalize`]
/// may create a plan from scratch for a directory with none, but must never
/// overwrite a file it couldn't parse (see [`JevExecutor::cached_plan`]).
#[derive(Clone)]
enum LoadedPlan {
    /// No `.check-plan.toml` exists yet for this directory.
    Missing,
    /// A `.check-plan.toml` exists but failed to parse (corrupt, or an
    /// unknown schema version) — already warned about, once, right where
    /// this is produced (see [`JevExecutor::get_or_load_plan`]).
    Corrupt,
    /// Successfully parsed.
    Loaded(Arc<PlanFile>),
}

impl LoadedPlan {
    /// Collapse [`Self::Missing`]/[`Self::Corrupt`] to `None` — the shape
    /// [`JevExecutor::decide`] actually needs (both decide `Agent`
    /// identically there).
    fn into_option(self) -> Option<Arc<PlanFile>> {
        match self {
            LoadedPlan::Loaded(plan) => Some(plan),
            LoadedPlan::Missing | LoadedPlan::Corrupt => None,
        }
    }
}

/// The [`OnceCell`] backing one directory's [`LoadedPlan`] — see
/// [`JevExecutor::plan_cache`]'s docs.
type PlanLoad = OnceCell<LoadedPlan>;

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
    /// `multi check --frozen` (MULTI-1826): disables self-healing entirely
    /// (no entry-building, no plan writes) — see the module docs. Does
    /// **not** disable the `ReadsStale` row's own live Jev consult, which
    /// decides this run's verdict, not a plan write.
    frozen: bool,
    /// Collects and eventually applies this run's self-healing plan updates
    /// (MULTI-1826) — see the module docs and [`healer::Healer`].
    healer: Healer,
}

impl JevExecutor {
    pub fn new(
        inner: Arc<dyn CheckExecutor + Send + Sync>,
        jev_client: Arc<JevClient>,
        jev_config: JevConfig,
        no_cache: bool,
        frozen: bool,
    ) -> Self {
        Self {
            healer: Healer::new(jev_client.clone(), jev_config.clone()),
            inner,
            jev_client,
            jev_config,
            no_cache,
            frozen,
            warned_no_root: Mutex::new(HashSet::new()),
            plan_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Load (and parse) the directory's `.check-plan.toml` at most once for
    /// this executor's whole lifetime — see [`Self::plan_cache`]'s docs.
    async fn get_or_load_plan(&self, dir: &Path) -> LoadedPlan {
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
                Ok(Some(plan)) => LoadedPlan::Loaded(Arc::new(plan)),
                Ok(None) => LoadedPlan::Missing,
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
                    LoadedPlan::Corrupt
                }
            }
        })
        .await
        .clone()
    }

    /// Read a directory's already-cached [`LoadedPlan`] without triggering a
    /// fresh load (MULTI-1826) — used only by [`Self::finalize`], which only
    /// ever asks about a directory [`Self::decide`] (or a retried
    /// [`Self::run_agent`], whose owning check's attempt 1 always went
    /// through `decide` first) has already consulted at least once this run
    /// — see [`healer::Healer`]'s and [`Self::run_agent`]'s docs. A
    /// directory with no cache entry at all is therefore unreachable here in
    /// practice; it defensively reads as [`LoadedPlan::Missing`] rather than
    /// panicking.
    fn cached_plan(&self, dir: &Path) -> LoadedPlan {
        let cache = self
            .plan_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache
            .get(dir)
            .and_then(|cell| cell.get())
            .cloned()
            .unwrap_or(LoadedPlan::Missing)
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
        let Some(plan) = self.get_or_load_plan(&req.plan.dir).await.into_option() else {
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
            Freshness::ReadsStale => self.decide_reads_stale(req, &entry, &replayed.calls).await,
        }
    }

    /// The `ReadsStale` branch: ask Jev the live "is this satisfied"
    /// question over the replayed evidence and decide from its answer — see
    /// the module docs' decision table and its note on a stored FAIL.
    ///
    /// `entry` is the full stored [`PlanCheck`] (not just its calibration):
    /// on a `Satisfied` settlement this also queues a self-healing refresh
    /// (MULTI-1826) onto [`Self::healer`] — see [`healer::Healer::queue_refresh`]'s
    /// docs on why that's safe to do synchronously, unlike an agent
    /// escalation's entry-building.
    async fn decide_reads_stale(
        &self,
        req: &AgentRunRequest,
        entry: &PlanCheck,
        replayed_calls: &[ReplayedCall],
    ) -> Result<Decision> {
        // Guaranteed `Some` by `Self::decide`'s caller: only a `Decider::Jev`
        // entry with a calibration ever reaches `ReadsStale` in the first
        // place (see `decide`'s own defensive check just above its replay
        // call).
        let calibration = entry
            .jev
            .as_ref()
            .expect("a `Decider::Jev` entry reaching `ReadsStale` always carries a calibration");

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
                if !self.frozen {
                    // Refresh the stored entry in place: checksums, verdict,
                    // and the calibrated `noul`, keeping `control_noul` — see
                    // `refreshed_entry`'s docs. No extra Jev call: the live
                    // consult above already produced everything needed.
                    let healed = refreshed_entry(entry, replayed_calls, noul);
                    self.healer.queue_refresh(&req.plan, healed);
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

    /// Run the agent (the one escalation target both `run_check` call sites
    /// share — see the module docs) and *record* this attempt's outcome for
    /// self-healing (MULTI-1826) before returning it — recording only, never
    /// building: [`healer::Healer::queue_agent_escalation`] does no I/O and
    /// spawns nothing (see the module docs on why that matters). Healing
    /// identity is captured from `req` **before** it's moved into
    /// `inner.run_check` (which acquires and, by the time it returns, has
    /// already torn down the sandbox `req.sandbox` leased) — see
    /// [`AgentOutcome::sandbox_root`]'s docs for how healing recovers the
    /// sandbox root itself, from the returned outcome, once it actually
    /// runs.
    ///
    /// Purely additive: the returned `Result<AgentOutcome>` is exactly what
    /// `self.inner.run_check(req)` produced — recording never touches it and
    /// never does any work of its own, so this can't change a verdict or
    /// delay settlement.
    async fn run_agent(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        let heal_ctx =
            (!self.frozen && req.plan.root_source == RootSource::Manifest).then(|| HealContext {
                identity: req.plan.clone(),
                check: req.check.clone(),
                declared_in: req.declared_in.clone(),
                root: req.source_dir.clone(),
            });

        let outcome = self.inner.run_check(req).await?;

        if let Some(ctx) = heal_ctx {
            self.healer.queue_agent_escalation(ctx, &outcome);
        }

        Ok(outcome)
    }
}

#[async_trait]
impl CheckExecutor for JevExecutor {
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome> {
        // Retries never redo Jev work — see the module docs. Still routed
        // through `run_agent` (not `self.inner.run_check` directly): a
        // retry's own trace is what MUST heal a stale entry when it
        // eventually reports (the ticket: "an agent run on attempt > 1
        // heals from that attempt's trace").
        if req.attempt > 1 {
            return self.run_agent(req).await;
        }

        match self.decide(&req).await? {
            Decision::Settled(outcome) => Ok(outcome),
            Decision::Agent => self.run_agent(req).await,
        }
    }

    /// Build and flush this run's self-healing plan updates (MULTI-1826) —
    /// see the module docs. This is where every queued escalation's replay
    /// and Jev calls actually happen — nothing before this point ever did
    /// any of that work. Called exactly once by `crate::checks::run`, and
    /// only when `crate::checks::run_pipeline` returned successfully (an
    /// aborted run never calls this at all — see that call site's docs), by
    /// which point every check has already settled and been reported: this
    /// can never touch a verdict or the run's exit code. Every failure here
    /// (a task that panicked, an entry that couldn't be built or timed out,
    /// a write that failed) is logged and skipped rather than propagated.
    async fn finalize(&self) {
        if self.frozen {
            return;
        }

        let updates_by_dir = self.healer.finish().await;
        if updates_by_dir.is_empty() {
            return;
        }

        let mut healed = 0usize;
        for (dir, updates) in updates_by_dir {
            match self.cached_plan(&dir) {
                LoadedPlan::Corrupt => {
                    tracing::warn!(
                        dir = %dir.display(),
                        "not self-healing this directory's .check-plan.toml: the existing \
                         file failed to parse; fix it (or regenerate with `multi plan`) first",
                    );
                }
                loaded => {
                    let base = loaded
                        .into_option()
                        .map(|plan| (*plan).clone())
                        .unwrap_or_else(|| PlanFile::new(Vec::new()));
                    let merged = apply_updates(base, updates);
                    match PlanStore::write(&dir, &merged) {
                        Ok(()) => healed += 1,
                        Err(err) => tracing::error!(
                            dir = %dir.display(),
                            error = %err,
                            "failed to write self-healed .check-plan.toml",
                        ),
                    }
                }
            }
        }

        if healed > 0 {
            tracing::info!(
                count = healed,
                "self-healed {healed} plan file(s) from this run's agent escalations",
            );
        }
    }
}

/// Merge every collected [`Update`] onto `base` — creating a new
/// [`PlanRequirement`] when an update's `(source, req_ordinal)` isn't in
/// `base` yet (a directory with no prior plan at all, or a requirement a
/// stale plan never covered), and replacing-or-appending each update's check
/// by `check_ordinal` within its requirement. Drops nothing else: every
/// requirement/check `base` already had that no update touches is carried
/// through unchanged. [`PlanStore::write`] re-sorts and dedups on its own, so
/// insertion order here doesn't matter.
fn apply_updates(mut base: PlanFile, updates: Vec<Update>) -> PlanFile {
    for update in updates {
        let req_index = base
            .requirements
            .iter()
            .position(|r| r.source == update.source && r.ordinal == update.req_ordinal)
            .unwrap_or_else(|| {
                base.requirements.push(PlanRequirement {
                    title: update.requirement_title.clone(),
                    source: update.source.clone(),
                    ordinal: update.req_ordinal,
                    checks: Vec::new(),
                });
                base.requirements.len() - 1
            });

        let requirement = &mut base.requirements[req_index];
        match requirement
            .checks
            .iter_mut()
            .find(|c| c.ordinal == update.check_ordinal)
        {
            Some(existing) => *existing = update.entry,
            None => requirement.checks.push(update.entry),
        }
    }
    base
}

/// Build the healed replacement for `entry` after a live `ReadsStale`
/// `Satisfied` settlement (MULTI-1826): every call's checksum(s) are
/// refreshed to this replay's fresh values (a [`PlanCall::Truncated`] call is
/// carried over unchanged — it has no checksum to refresh), the verdict
/// flips to satisfied, and the calibration's `noul` is updated to the live
/// call's — but `control_noul`/`model`/`reading` are all kept exactly as
/// calibrated at plan time: the negative control depends only on the prompt
/// and the model, neither of which changed here (the ticket).
///
/// `evidence` is also refreshed — a design decision, not spelled out
/// verbatim in the ticket: the stored text described whatever the *previous*
/// real verdict was (which may have been a FAIL), and leaving it as-is here
/// would freeze a passing check together with a failing explanation for
/// every future `Cached` run. This mirrors [`jev_settled_outcome`]'s own
/// reasoning for *this* run's outcome — see the module docs' note on a
/// stored FAIL.
///
/// `entry.calls` and `replayed_calls` are guaranteed the same length and
/// order (the latter is `replay::replay_check`'s replay of the former), and
/// — because this is only ever reached once a check's aggregated freshness
/// is `ReadsStale`, never `DiscoveryStale` — every non-[`PlanCall::Truncated`]
/// call here is guaranteed a fresh checksum to convert
/// ([`ReplayedCall::to_plan_call`] returning `None` is unreachable; kept as
/// a defensive fallback to the stored call rather than a panic).
fn refreshed_entry(entry: &PlanCheck, replayed_calls: &[ReplayedCall], noul: f64) -> PlanCheck {
    let calls = entry
        .calls
        .iter()
        .zip(replayed_calls)
        .map(|(stored, replayed)| match stored {
            PlanCall::Truncated { tool, input } => PlanCall::Truncated {
                tool: *tool,
                input: input.clone(),
            },
            _ => replayed.to_plan_call().unwrap_or_else(|| stored.clone()),
        })
        .collect();

    let jev = entry.jev.clone().map(|calibration| JevCalibration {
        noul,
        ..calibration
    });
    let model = jev.as_ref().map_or("", |j| j.model.as_str());

    PlanCheck {
        title: entry.title.clone(),
        ordinal: entry.ordinal,
        prompt_xxh64: entry.prompt_xxh64.clone(),
        decider: Decider::Jev,
        verdict: true,
        evidence: Some(jev_satisfied_evidence(noul, model)),
        jev,
        calls,
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
        // No agent ran, so no sandbox was ever acquired — see
        // `AgentOutcome::sandbox_root`'s docs.
        sandbox_root: None,
    }
}

/// The explanation text for a settlement Jev affirmatively demonstrated —
/// shared by [`jev_settled_outcome`] (this run's returned outcome) and
/// [`refreshed_entry`] (the healed plan entry for future runs), which must
/// agree: see [`refreshed_entry`]'s docs on why the stored entry's evidence
/// is refreshed to this same text rather than left stale.
fn jev_satisfied_evidence(noul: f64, model: &str) -> String {
    format!(
        "Jev verified the replayed evidence affirmatively demonstrates this check is satisfied (noul {noul:.3}, model {model})"
    )
}

/// Build a `Jev`-settled, satisfied [`AgentOutcome`] from a live Jev call —
/// see the module docs' note on a stored FAIL for why this is always
/// `success: true` (only [`JevDecision::Satisfied`] ever reaches this) and
/// why it never reuses the plan's stored `evidence` text.
fn jev_settled_outcome(noul: f64, model: &str) -> AgentOutcome {
    AgentOutcome {
        verdict: Some(CheckReport {
            success: true,
            evidence: Some(jev_satisfied_evidence(noul, model)),
        }),
        stop_reason: Some("settled by Jev".to_string()),
        turns: 0,
        error: None,
        trace_jsonl: None,
        tool_calls: Vec::new(),
        decided_by: DecidedBy::Jev,
        // No agent ran, so no sandbox was ever acquired — see
        // `AgentOutcome::sandbox_root`'s docs.
        sandbox_root: None,
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
