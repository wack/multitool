//! `multi plan` (MULTI-1824): establish what evidence is necessary to verify
//! each check and freeze it into `.check-plan.toml`.
//!
//! This is a **separate** subcommand and code path from `multi check` — see
//! `crate::checks`' own module docs. It reuses [`discovery::discover`]'s
//! walk/parse/validate function directly (not `DiscoveryActor`), so it gets
//! the exact same strict whole-run abort on an invalid `CHECKS.toml`, and
//! reuses [`crate::checks::executor::CheckExecutor`]/
//! [`crate::checks::sandbox::Sandbox`] the same way `multi check` does — but
//! does **not** reuse the actor pipeline. [`Planner`] is this module's own DI
//! seam, and [`AgentPlanner`] is the real implementation — see `planner`'s
//! module docs for the calibration logic.
//!
//! ## Live UI (MULTI-1829)
//!
//! Like `multi check`, `multi plan` drives a display-only, event-driven
//! presenter — but its **own** event stream ([`presenter::PlanUiEvent`]),
//! folded into its own state machine ([`presenter::PlanPresenterState`]) and
//! rendered by its own backends, so a plan-specific row state (`Verifying`,
//! `Stale{reason}`, `Calibrating`, …) never has to be squeezed into `multi
//! check`'s [`crate::checks::presenter::UiEvent`]/`CheckState`. The event
//! sink ([`presenter::PlanEventSink`]) is threaded, optionally, through
//! [`run_with_planner`] → [`process_check`] → [`planner::PlanRequest`] →
//! [`AgentPlanner`] — `None` in every orchestration test in this module, so
//! their assertions are unaffected. This **replaces** the interim
//! one-line-per-check output this module shipped as: see
//! [`run_with_planner`]'s docs for the plain-record fallback printed when
//! there is no TTY-owning live backend.
//!
//! ## Repository roots
//!
//! A requirements file whose [`Requirement::root_source`] is
//! [`RootSource::ScanDirectory`] (no `MultiTool.toml` manifest above it,
//! MULTI-1834) is **refused**: its scope isn't stable across invocations, so
//! freezing evidence relative to it would silently break the moment `multi
//! plan` is run from a different directory. Every check under a refused file
//! is reported `error`, and no `.check-plan.toml` is written for it; other
//! files in the same run are planned normally.
//!
//! ## Caching
//!
//! Before invoking the planner for a check, its existing plan entry (looked
//! up by `(requirement id, check id, prompt_xxh64)` — see
//! [`plan_file::PlanFile::lookup`]) is replayed
//! ([`replay::replay_check`]). A [`Freshness::Fresh`] result reuses the entry
//! **untouched**: zero agent runs, zero Jev calls. `--force` skips this
//! lookup entirely (every check is (re)planned from scratch). An existing
//! `.check-plan.toml` that fails to load (corrupt file, unknown schema
//! version) aborts the whole run rather than silently discarding it — a
//! `PlanError` is itself a `miette::Diagnostic`, so it propagates via `?`
//! with no extra wrapping.
//!
//! ## Concurrency and writing
//!
//! Every check across every non-refused directory is dispatched through one
//! bounded-concurrency stream (`checks.concurrency`), regardless of which
//! `.check-plan.toml` it belongs to. Results are collected, then each
//! directory's plan is written **once**, at the end, via
//! [`plan_file::PlanStore::write`] — never a concurrent writer per file.
//!
//! ## Output
//!
//! Exit code `0` unless some check could not be planned (a refused file's
//! checks count as not-planned too). The live view and the final record
//! (scrollback, or plain stdout when there's no TTY-owning backend) are
//! covered by [`presenter`]; [`RunReport`]'s numeric fields (`exit_code`,
//! `truncated_count`, `total_checks`) are this module's stable, directly
//! testable contract (MULTI-1824's own tests assert them; MULTI-1829 doesn't
//! change their meaning).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use indexmap::IndexMap;
use kameo::actor::Spawn;
use miette::Result;

use crate::Terminal;
use crate::checks::config::{self, CliOverrides};
use crate::checks::discovery;
use crate::checks::executor::CheckExecutor;
use crate::checks::jev::client::JevClient;
use crate::checks::jev::plan_file::{self, AgentReason, Decider, PlanCall};
use crate::checks::jev::replay::{self, Freshness};
use crate::checks::model::{Check, Requirement, RootSource};
use crate::checks::sandbox::{self, Sandbox};

mod planner;
pub(crate) mod presenter;

pub use planner::{AgentPlanner, PlanRequest, PlannedCheck, Planner};
// `BoxedPlanner` stays reachable only at `planner::BoxedPlanner` for now — no
// caller outside `planner.rs` itself needs it yet (`AgentPlanner` uses
// `Arc<dyn Planner>`, never `Box`, throughout this module), and re-exporting
// it here today would be an unused `pub use` in this privately-rooted module
// tree (`mod checks;` in `lib.rs` is not `pub`, so nothing outside the crate
// can ever reach a `pub` item here either) — widen this the moment something
// outside `planner.rs` needs it.
//
// `entry_from_outcome`/`EntryContext` ARE re-exported flat: MULTI-1826's
// self-healing (`multi check` escalation, `crate::checks::jev::executor`)
// calls `entry_from_outcome` from outside this module (see its own docs on
// why it's the single entry-builder both callers share) — the same situation
// `crate::checks::config`'s `JevConfig`/`resolve_jev` re-export was in before
// MULTI-1824 became their first cross-module consumer.
#[cfg(test)]
pub(crate) use planner::fake::FakePlanner;
pub(crate) use planner::{EntryContext, entry_from_outcome};

/// Run `multi plan` rooted at `working_dir`. Builds the real [`AgentPlanner`]
/// (composing the same raw agent executor/[`Sandbox`] `multi check` composes
/// as its own `JevExecutor`'s inner escalation target — see
/// [`config::Resolved::build_agent_executor`]'s docs on why `multi plan`
/// calls that directly rather than [`config::Resolved::build_executor`]) and
/// delegates to [`run_with_planner`], the injectable core the tests drive
/// directly with a [`FakePlanner`].
///
/// Spawns the live presenter up front (MULTI-1829), exactly like
/// `crate::checks::run`: the inline TUI when stdout is a TTY (falling back to
/// the heartbeat if terminal setup fails), otherwise the stderr heartbeat.
/// The presenter is stopped — and, for the inline TUI, its terminal restored
/// — regardless of whether the run below succeeds, errors per-check, or
/// aborts outright (a bad/missing Jev credential): display must never be the
/// reason a terminal is left in a broken state. That stop is bounded
/// (MULTI-1829 code review: `presenter::shutdown` times out and kills a
/// stuck presenter) so a stalled/dead presenter can never hang the process
/// after `run_with_planner` has already finished all planning work.
///
/// Returns the process exit code (mirroring `crate::checks::run`'s own
/// contract): `0` unless a check could not be planned. An invalid suite (a
/// malformed `CHECKS.toml`) or a Jev credential/request failure abort the run
/// as `Err` rather than reporting a numeric code.
pub async fn run(
    terminal: &Terminal,
    working_dir: &Path,
    overrides: CliOverrides,
    force: bool,
    sandbox_enabled: bool,
) -> Result<i32> {
    // Two separate merges over the same `flag > env > file` layers: `load`
    // resolves everything `multi check` also resolves (and validates
    // identically to it — see that function's docs), and `load_jev` resolves
    // `[checks.jev]` alone. Kept apart rather than folded into one `Resolved`
    // so `multi check`'s own `load` call never has to validate
    // `checks.jev.threshold` — see `config::load_jev`'s docs.
    let resolved = config::load(overrides.clone())?;
    let jev_config = config::load_jev(overrides)?;
    let requirements = discovery::discover(working_dir).await?;

    let executor: Arc<dyn CheckExecutor + Send + Sync> = Arc::new(resolved.build_agent_executor());
    let sandbox: Arc<dyn Sandbox + Send + Sync> =
        Arc::from(sandbox::select_sandbox(sandbox_enabled));
    let jev_client = Arc::new(JevClient::from_config(&jev_config)?);
    let planner: Arc<dyn Planner + Send + Sync> = Arc::new(AgentPlanner::new(
        executor,
        sandbox,
        jev_client,
        jev_config,
        resolved.config.max_attempts,
    ));

    let presenter::Backend {
        backend,
        owns_record,
    } = presenter::select_backend(terminal.stdout_allows_color());
    let final_record = presenter::FinalRecordSlot::new();
    let presenter_actor = presenter::PlanPresenterActor::spawn(presenter::PlanPresenterActor::new(
        backend,
        resolved.config.model.clone(),
        final_record.clone(),
    ));
    let sink = presenter::PlanEventSink::new(presenter_actor.clone());

    let result = run_with_planner(
        terminal,
        &requirements,
        planner,
        resolved.config.concurrency,
        force,
        Some(presenter::Presentation {
            sink,
            owns_record,
            final_record,
        }),
    )
    .await;

    // Always stop the presenter — even on an aborting `Err` — so a TTY
    // backend restores the terminal regardless of outcome. `presenter::shutdown`
    // bounds the wait (MULTI-1829 code review item 2): the presenter is
    // display-only, so its shutdown is never allowed to affect (or hang)
    // `result`.
    presenter::shutdown(&presenter_actor).await;

    let report = result?;
    Ok(report.exit_code)
}

// ---------------------------------------------------------------------------
// Grouping: requirements -> one `.check-plan.toml` per directory
// ---------------------------------------------------------------------------

/// Every requirement declared in one directory's `CHECKS.toml`, sharing one
/// `.check-plan.toml` — see the module docs. Requirements within a group are
/// identified by their ids (unique within the file);
/// `root`/`root_source` are identical for every requirement in a group by
/// construction (both are resolved from the same directory — see
/// `discovery::repo_root::resolve`).
struct FileGroup<'a> {
    root: PathBuf,
    root_source: RootSource,
    requirements: Vec<&'a Requirement>,
}

/// Group `requirements` by the directory containing their declaring file —
/// see [`crate::checks::model::plan_dir`], shared with `multi check`'s
/// execution (MULTI-1825) so both commands agree on where a requirement's
/// plan lives. Insertion-ordered ([`IndexMap`]) so output/writing stays in
/// discovery order.
fn group_by_directory(requirements: &[Requirement]) -> IndexMap<PathBuf, FileGroup<'_>> {
    let mut groups: IndexMap<PathBuf, FileGroup<'_>> = IndexMap::new();
    for req in requirements {
        let dir = crate::checks::model::plan_dir(&req.filepath);
        let group = groups.entry(dir).or_insert_with(|| FileGroup {
            root: req.root.clone(),
            root_source: req.root_source,
            requirements: Vec::new(),
        });
        group.requirements.push(req);
    }
    groups
}

// ---------------------------------------------------------------------------
// Per-check task descriptors
// ---------------------------------------------------------------------------

/// One check's static planning context — everything needed to look it up in
/// an existing plan, run it through the planner if needed, and place its
/// result back into the right `.check-plan.toml`.
struct CheckDescriptor {
    /// This check's position in the flat dispatch list — restores
    /// deterministic (discovery) order after the bounded-concurrency stream
    /// completes, doubles as the [`crate::checks::model::CheckId`]
    /// `AgentRunRequest` wants for session-id namespacing, and (MULTI-1829)
    /// is this check's row id in the live presenter's tree.
    index: usize,
    /// This check's requirement's position for the live presenter's tree
    /// (MULTI-1829) — assigned in traversal order over [`group_by_directory`]'s
    /// output (directory discovery order, then declaration order within each
    /// directory's file), **not** necessarily the flat requirements-file
    /// declaration order `multi check`'s own `req_index` uses; nothing in
    /// this module needs the two to agree, only that grouping is stable and
    /// unique within one run — see [`assign_req_indices`].
    req_index: usize,
    dir: PathBuf,
    requirement_id: String,
    requirement_title: String,
    check: Check,
    root: PathBuf,
    declared_in: PathBuf,
}

/// One refused-file check: no descriptor was ever built for it (its file has
/// no manifest-derived root), but it still needs an `error` line and to count
/// toward the summary/exit code.
struct RefusedCheck {
    req_index: usize,
    requirement_title: String,
    check_title: String,
    message: String,
}

/// Assign each `(dir, requirement_id)` requirement seen while walking
/// `groups` a sequential id, in traversal order — the presenter-tree grouping
/// key [`CheckDescriptor::req_index`]/[`RefusedCheck::req_index`] use. Covers
/// **every** group (refused and valid alike), so a refused requirement gets
/// its own stable id too and can still render a row in the live tree.
fn assign_req_indices(
    groups: &IndexMap<PathBuf, FileGroup<'_>>,
) -> HashMap<(PathBuf, String), usize> {
    let mut map = HashMap::new();
    let mut next = 0usize;
    for (dir, group) in groups {
        for req in &group.requirements {
            map.entry((dir.clone(), req.id.clone())).or_insert_with(|| {
                let id = next;
                next += 1;
                id
            });
        }
    }
    map
}

/// Partition `groups` into plannable [`CheckDescriptor`]s (one per check,
/// across every group whose root is manifest-derived) and [`RefusedCheck`]s
/// (one per check under a [`RootSource::ScanDirectory`] group) — see the
/// module docs' "Repository roots" section. Also returns every valid
/// directory seen, even one that (after processing) ends up with zero
/// surviving checks, so [`run_with_planner`] still writes an (empty) plan for
/// it rather than leaving a stale file untouched.
fn partition_checks(
    groups: &IndexMap<PathBuf, FileGroup<'_>>,
) -> (Vec<CheckDescriptor>, Vec<RefusedCheck>, Vec<PathBuf>) {
    let req_indices = assign_req_indices(groups);
    let mut descriptors = Vec::new();
    let mut refused = Vec::new();
    let mut valid_dirs = Vec::new();

    for (dir, group) in groups {
        if group.root_source == RootSource::ScanDirectory {
            for req in &group.requirements {
                let req_index = req_indices[&(dir.clone(), req.id.clone())];
                for check in &req.checks {
                    refused.push(RefusedCheck {
                        req_index,
                        requirement_title: req.title.clone(),
                        check_title: check.title.clone(),
                        message: refused_message(&req.filepath),
                    });
                }
            }
            continue;
        }

        valid_dirs.push(dir.clone());
        for req in &group.requirements {
            let req_index = req_indices[&(dir.clone(), req.id.clone())];
            let declared_in =
                crate::checks::execution::declared_in_relative_to_root(&req.filepath, &group.root);
            for check in &req.checks {
                let index = descriptors.len();
                descriptors.push(CheckDescriptor {
                    index,
                    req_index,
                    dir: dir.clone(),
                    requirement_id: req.id.clone(),
                    requirement_title: req.title.clone(),
                    check: check.clone(),
                    root: group.root.clone(),
                    declared_in: declared_in.clone(),
                });
            }
        }
    }

    (descriptors, refused, valid_dirs)
}

/// The diagnostic recorded for every check under a [`RootSource::ScanDirectory`]
/// requirements file — see the module docs' "Repository roots" section.
/// Names the offending file and tells the user how to fix it; pulled out as
/// its own pure function so it's directly unit-testable.
fn refused_message(filepath: &Path) -> String {
    format!(
        "no MultiTool.toml manifest found above `{}`; add one at the repository root to plan this file",
        filepath.display(),
    )
}

// ---------------------------------------------------------------------------
// Caching: load existing plans, look up cached entries
// ---------------------------------------------------------------------------

/// Load the existing `.check-plan.toml` (if any) beside every directory in
/// `valid_dirs`, once each — **unconditionally**, regardless of `--force`.
/// Besides the freshness *lookup* ([`cached_entry`], which `--force` does
/// bypass), [`build_plan_files`] also needs this to preserve an existing
/// entry for a check whose (re-)planning fails this run rather than deleting
/// it: `--force` means "re-plan", not "delete on failure" — see the module
/// docs. A load failure (corrupt file, unknown version) aborts the whole run
/// rather than silently discarding whatever was already committed.
fn load_existing_plans(valid_dirs: &[PathBuf]) -> Result<HashMap<PathBuf, plan_file::PlanFile>> {
    let mut out = HashMap::new();
    for dir in valid_dirs {
        if let Some(loaded) = plan_file::PlanStore::load(dir)? {
            out.insert(dir.clone(), loaded);
        }
    }
    Ok(out)
}

/// The existing plan entry for `descriptor`, if `existing` has a plan for its
/// directory and that plan has a matching, hash-valid entry — `None`
/// unconditionally when `force` is set, so every check is (re)planned from
/// scratch regardless of what `existing` holds.
fn cached_entry(
    descriptor: &CheckDescriptor,
    existing: &HashMap<PathBuf, plan_file::PlanFile>,
    force: bool,
) -> Option<plan_file::PlanCheck> {
    if force {
        return None;
    }
    let plan = existing.get(&descriptor.dir)?;
    let hash = plan_file::prompt_xxh64(&descriptor.check.title, descriptor.check.prompt());
    plan.lookup(&descriptor.requirement_id, &descriptor.check.id, &hash)
        .cloned()
}

/// Everything [`process_check`] needs to know about a check's plan-cache
/// status before any replay IO runs (MULTI-1829): the hash-matching entry to
/// attempt replay against (`None` under `--force`, exactly like
/// [`cached_entry`]), whether `--force` is what made it `None`, and — only
/// meaningful when `matched` is `None` and `forced` is `false` — whether
/// *some* entry existed for this check's ids regardless of hash, which is
/// what tells [`presenter::StaleReason::New`] apart from
/// [`presenter::StaleReason::PromptChanged`].
struct CacheLookup {
    matched: Option<plan_file::PlanCheck>,
    existed: bool,
    forced: bool,
}

fn cache_lookup(
    descriptor: &CheckDescriptor,
    existing: &HashMap<PathBuf, plan_file::PlanFile>,
    force: bool,
) -> CacheLookup {
    let existed = existing
        .get(&descriptor.dir)
        .and_then(|plan| plan.find(&descriptor.requirement_id, &descriptor.check.id))
        .is_some();
    CacheLookup {
        matched: cached_entry(descriptor, existing, force),
        existed,
        forced: force,
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// A check's terminal result: reused from cache, freshly planned (`decider`
/// distinguishes `planned` from `agent-only` at output time), or errored.
enum LineOutcome {
    Reused(plan_file::PlanCheck),
    Planned(PlannedCheck),
    Error(miette::Report),
}

struct ProcessedCheck {
    descriptor: CheckDescriptor,
    outcome: LineOutcome,
    /// Why this check was (re-)planned this run — `None` for a `Reused`
    /// outcome (never stale) or when `plan_it` was never reached at all.
    /// Carried alongside `outcome` (rather than folded into `LineOutcome`
    /// itself) purely so [`build_final_record`] (MULTI-1829 code review item
    /// 3) can show it for both `Planned` and `AgentOnly` terminal outcomes
    /// without `LineOutcome` needing to know anything about presentation.
    stale_reason: Option<presenter::StaleReason>,
}

/// Fire-and-forget `event` at `sink` if there is one — the one-line helper
/// every event-emission call site in this module uses so the `if let
/// Some(sink) = sink { ... }` boilerplate doesn't repeat at every call site.
/// Non-async/non-blocking (MULTI-1829 code review item 1): delivery is
/// best-effort — see [`presenter::PlanEventSink::send`]'s docs — so this
/// never gates cache verification, agent start/retry, calibration, or the
/// final plan write.
fn tell(sink: Option<&presenter::PlanEventSink>, event: presenter::PlanUiEvent) {
    if let Some(sink) = sink {
        sink.send(event);
    }
}

/// Resolve one check: replay its cached entry (if any) and reuse it when
/// still [`Freshness::Fresh`]; otherwise call the planner. Emits the
/// `Verifying`/`Fresh`/`Stale{reason}` presenter milestones (MULTI-1829)
/// around the cache-lookup/replay step; [`plan_it`] emits the rest.
async fn process_check(
    descriptor: CheckDescriptor,
    lookup: CacheLookup,
    planner: Arc<dyn Planner + Send + Sync>,
    sink: Option<&presenter::PlanEventSink>,
) -> ProcessedCheck {
    let id = descriptor.index;

    if lookup.forced {
        tell(
            sink,
            presenter::PlanUiEvent::Stale {
                id,
                reason: presenter::StaleReason::Forced,
            },
        );
        return plan_it(descriptor, planner, sink, presenter::StaleReason::Forced).await;
    }

    tell(sink, presenter::PlanUiEvent::Verifying { id });

    if let Some(entry) = lookup.matched {
        let replayed =
            replay::replay_check(&descriptor.check.title, &entry.calls, &descriptor.root).await;
        if replayed.freshness == Freshness::Fresh {
            let truncated = calls_have_truncated(&entry.calls);
            tell(sink, presenter::PlanUiEvent::Fresh { id, truncated });
            return ProcessedCheck {
                outcome: LineOutcome::Reused(entry),
                descriptor,
                stale_reason: None,
            };
        }
        let reason = match replayed.freshness {
            Freshness::ReadsStale => presenter::StaleReason::FilesChanged,
            Freshness::DiscoveryStale => presenter::StaleReason::FileSetChanged,
            Freshness::Fresh => unreachable!("the Fresh case returned above"),
        };
        tell(sink, presenter::PlanUiEvent::Stale { id, reason });
        return plan_it(descriptor, planner, sink, reason).await;
    }

    let reason = if lookup.existed {
        presenter::StaleReason::PromptChanged
    } else {
        presenter::StaleReason::New
    };
    tell(sink, presenter::PlanUiEvent::Stale { id, reason });
    plan_it(descriptor, planner, sink, reason).await
}

/// Call the planner and emit the terminal presenter milestone
/// (`Planned`/`AgentOnly`/`Error`, MULTI-1829) for whatever it returns —
/// shared by every path [`process_check`] falls through to the planner from
/// (no cached entry, a stale cached entry, or `--force`). `stale_reason` is
/// carried into the returned [`ProcessedCheck`] so the final record can show
/// it later (MULTI-1829 code review item 3) — it is *not* threaded to the
/// planner itself (`PlanRequest` has no use for it).
async fn plan_it(
    descriptor: CheckDescriptor,
    planner: Arc<dyn Planner + Send + Sync>,
    sink: Option<&presenter::PlanEventSink>,
    stale_reason: presenter::StaleReason,
) -> ProcessedCheck {
    let id = descriptor.index;
    let req = PlanRequest {
        check_id: descriptor.index,
        check: descriptor.check.clone(),
        requirement_title: descriptor.requirement_title.clone(),
        declared_in: descriptor.declared_in.clone(),
        root: descriptor.root.clone(),
        plan_dir: descriptor.dir.clone(),
        requirement_id: descriptor.requirement_id.clone(),
        // Every descriptor here comes from a manifest-derived group — a
        // `RootSource::ScanDirectory` file's checks are refused before a
        // `CheckDescriptor` is ever built (see `partition_checks`).
        root_source: RootSource::Manifest,
        sink: sink.cloned(),
    };
    let outcome = match planner.plan_check(req).await {
        Ok(planned) => {
            let truncated = calls_have_truncated(&planned.calls);
            match planned.decider {
                Decider::Jev => {
                    tell(
                        sink,
                        presenter::PlanUiEvent::Planned {
                            id,
                            verdict: planned.verdict,
                            truncated,
                        },
                    );
                }
                Decider::Agent(reason) => {
                    tell(
                        sink,
                        presenter::PlanUiEvent::AgentOnly {
                            id,
                            reason,
                            verdict: planned.verdict,
                            truncated,
                        },
                    );
                }
            }
            LineOutcome::Planned(planned)
        }
        Err(err) => {
            tell(
                sink,
                presenter::PlanUiEvent::Error {
                    id,
                    message: err.to_string(),
                },
            );
            LineOutcome::Error(err)
        }
    };
    ProcessedCheck {
        descriptor,
        outcome,
        stale_reason: Some(stale_reason),
    }
}

/// The result of one `multi plan` run: the process exit code plus the
/// summary counts a caller can inspect directly — used by tests to assert
/// on the truncated-discovery count without capturing stdout (see
/// [`summary_line`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunReport {
    pub exit_code: i32,
    /// How many checks' final entry carries at least one truncated call —
    /// see [`entry_has_truncated_call`].
    pub truncated_count: usize,
    /// Every check considered this run, including refused-file checks.
    pub total_checks: usize,
}

/// The injectable core of `multi plan`, driven directly by tests with a
/// [`FakePlanner`] (or a real [`AgentPlanner`] over a `FakeExecutor` + mock
/// Jev). See the module docs for the full contract.
///
/// `presentation` (MULTI-1829) is `None` in every test in this module — the
/// live view is purely additive display, so its absence changes nothing
/// about what's planned, written, or the exit code. When `Some` but its
/// backend doesn't own the terminal record (the heartbeat backend, or a
/// failed TTY setup that already fell back — see `presenter::select_backend`),
/// and equally when `presentation` is `None` outright, this function prints
/// the plain final record to stdout itself: the requirement tree with every
/// check's terminal state, the stale reason for every re-planned check, and
/// the list of `.check-plan.toml` files written — the same content the
/// inline TUI flushes to scrollback, just as plain text. This **replaces**
/// the interim one-line-per-check dump this module used to print
/// unconditionally.
pub(crate) async fn run_with_planner(
    terminal: &Terminal,
    requirements: &[Requirement],
    planner: Arc<dyn Planner + Send + Sync>,
    concurrency: usize,
    force: bool,
    presentation: Option<presenter::Presentation>,
) -> Result<RunReport> {
    if requirements.is_empty() {
        terminal.write_stdout_line("No requirements found.")?;
        return Ok(RunReport {
            exit_code: 0,
            truncated_count: 0,
            total_checks: 0,
        });
    }

    let sink = presentation.as_ref().map(|p| &p.sink);
    let owns_record = presentation.as_ref().is_some_and(|p| p.owns_record);

    let groups = group_by_directory(requirements);
    let (descriptors, refused, valid_dirs) = partition_checks(&groups);
    // Loaded unconditionally, even under `--force` — see the doc comment.
    let existing = load_existing_plans(&valid_dirs)?;

    // Every discovered check — refused-file checks included — is announced
    // up front (MULTI-1829: "the full list of requirements is visible up
    // front"), before any cache lookup or planning begins.
    for d in &descriptors {
        tell(
            sink,
            presenter::PlanUiEvent::Queued {
                id: d.index,
                req_index: d.req_index,
                req_title: d.requirement_title.clone(),
                check_title: d.check.title.clone(),
            },
        );
    }
    let refused_ids: Vec<usize> = (descriptors.len()..).take(refused.len()).collect();
    for (r, id) in refused.iter().zip(refused_ids.iter().copied()) {
        tell(
            sink,
            presenter::PlanUiEvent::Queued {
                id,
                req_index: r.req_index,
                req_title: r.requirement_title.clone(),
                check_title: r.check_title.clone(),
            },
        );
        tell(
            sink,
            presenter::PlanUiEvent::Error {
                id,
                message: r.message.clone(),
            },
        );
    }
    tell(
        sink,
        presenter::PlanUiEvent::DiscoveryComplete {
            total_checks: descriptors.len() + refused.len(),
        },
    );

    let lookups: Vec<CacheLookup> = descriptors
        .iter()
        .map(|d| cache_lookup(d, &existing, force))
        .collect();

    let processed: Vec<ProcessedCheck> = stream::iter(descriptors.into_iter().zip(lookups))
        .map(|(descriptor, lookup)| {
            let planner = Arc::clone(&planner);
            let sink = sink.cloned();
            async move { process_check(descriptor, lookup, planner, sink.as_ref()).await }
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    // Abort the whole run on the first `AbortPlanRun`-marked failure — a bad
    // or missing Jev credential must not silently read as every check
    // disagreeing with the agent. No plan file is written in this case
    // (checked before any write below).
    let abort_index = processed.iter().position(|p| {
        matches!(&p.outcome, LineOutcome::Error(e) if e.downcast_ref::<planner::AbortPlanRun>().is_some())
    });
    if let Some(index) = abort_index {
        let ProcessedCheck { outcome, .. } = processed
            .into_iter()
            .nth(index)
            .expect("index came from this same Vec");
        let LineOutcome::Error(report) = outcome else {
            unreachable!("matched above")
        };
        return Err(report);
    }

    let mut processed = processed;
    processed.sort_by_key(|p| p.descriptor.index);

    let truncated_count = processed
        .iter()
        .filter(|p| entry_has_truncated_call(&p.outcome))
        .count();
    let total_checks = refused.len() + processed.len();

    let has_error = !refused.is_empty()
        || processed
            .iter()
            .any(|p| matches!(p.outcome, LineOutcome::Error(_)));

    // -- write: one `.check-plan.toml` per directory, exactly once --------
    // Seeded from `existing` so a check whose (re-)planning errored this run
    // keeps its previous entry instead of losing it — see `build_plan_files`.
    let mut files = build_plan_files(&processed, &existing);
    for dir in &valid_dirs {
        files
            .entry(dir.clone())
            .or_insert_with(|| plan_file::PlanFile::new(vec![]));
    }
    for (dir, file) in &files {
        plan_file::PlanStore::write(dir, file)?;
    }
    let mut written_paths: Vec<PathBuf> = files
        .keys()
        .map(|dir| dir.join(plan_file::PLAN_FILE_NAME))
        .collect();
    written_paths.sort();

    // Build this run's one, authoritative final record (MULTI-1829 code
    // review item 4) and deposit it directly into the slot the presenter
    // reads at teardown — bypassing the (lossy, best-effort) event mailbox
    // entirely, so it reaches the presenter intact even if every single
    // `PlanUiEvent` this run ever sent was dropped. See `presenter::FinalRecord`'s
    // docs.
    let final_record = build_final_record(
        &refused,
        &processed,
        &written_paths,
        truncated_count,
        total_checks,
    );
    if let Some(p) = &presentation {
        p.final_record.set(final_record.clone());
    }

    if !owns_record {
        print_plain_record(terminal, &final_record)?;
    }

    Ok(RunReport {
        exit_code: if has_error { 1 } else { 0 },
        truncated_count,
        total_checks,
    })
}

/// Whether a processed check's final entry carries at least one
/// [`PlanCall::Truncated`] call — true for a fresh `agent-only (truncated
/// discovery)` entry, and equally true for a `reused` entry whose frozen
/// calls already included one (truncated calls are excluded from freshness,
/// so such an entry is `reused`, not replanned — see the module docs).
fn entry_has_truncated_call(outcome: &LineOutcome) -> bool {
    let calls: &[PlanCall] = match outcome {
        LineOutcome::Reused(entry) => &entry.calls,
        LineOutcome::Planned(planned) => &planned.calls,
        LineOutcome::Error(_) => return false,
    };
    calls_have_truncated(calls)
}

/// Whether any call in `calls` is a [`PlanCall::Truncated`] — the shared
/// predicate [`entry_has_truncated_call`] and the presenter-event emission in
/// [`process_check`]/[`plan_it`] both use, so "this row is truncated" can
/// never drift between the summary count and the live/final record.
fn calls_have_truncated(calls: &[PlanCall]) -> bool {
    calls
        .iter()
        .any(|c| matches!(c, PlanCall::Truncated { .. }))
}

// ---------------------------------------------------------------------------
// Assembling and writing `.check-plan.toml` files
// ---------------------------------------------------------------------------

/// A requirement's checks under construction: title plus the checks
/// assembled for it so far.
type RequirementChecks = (String, Vec<plan_file::PlanCheck>);
/// One directory's requirements under construction, keyed by requirement id
/// — see [`build_plan_files`].
type DirectoryRequirements = HashMap<String, RequirementChecks>;

/// Assemble one [`plan_file::PlanFile`] per directory from `processed`.
///
/// A check successfully `Reused` or freshly `Planned` this run contributes
/// its new entry. A check whose planning **errored** this run does *not*
/// contribute nothing — that used to be this function's behavior, and it was
/// a bug (MULTI-1824 review): omitting it here meant a single transient
/// failure (a Jev `Transport`/`Exhausted` blip, an agent that never reports,
/// or the escaping-call error `entry_from_outcome` now raises) silently
/// **deleted** whatever entry that check already had, destroying its cached
/// verdict/calibration for no reason connected to that entry's own
/// freshness. The ticket only calls for dropping entries for checks no
/// longer present in `CHECKS.toml` at all — which this function never even
/// sees, since `processed` is built from the *current* discovery pass (see
/// [`partition_checks`]), not from `existing`. So an errored check instead
/// looks up whatever entry `existing` already had for its exact
/// `(requirement id, check id)` ([`plan_file::PlanFile::find`], deliberately
/// **not** gated on `prompt_xxh64` still matching — an entry
/// preserved this way may be stale, and that's fine: `multi check` handles a
/// stale entry safely by replaying and escalating; a *missing* one loses the
/// cached verdict/calibration outright). This preservation applies
/// regardless of `--force`: force means "re-plan", not "delete on failure".
///
/// [`plan_file::PlanStore::write`] re-sorts requirements/checks
/// deterministically on its own, so insertion order here doesn't matter.
fn build_plan_files(
    processed: &[ProcessedCheck],
    existing: &HashMap<PathBuf, plan_file::PlanFile>,
) -> HashMap<PathBuf, plan_file::PlanFile> {
    let mut by_dir: HashMap<PathBuf, DirectoryRequirements> = HashMap::new();

    for p in processed {
        let entry = match &p.outcome {
            LineOutcome::Reused(entry) => Some(entry.clone()),
            LineOutcome::Planned(planned) => Some(planned.clone().into_plan_check()),
            LineOutcome::Error(_) => existing.get(&p.descriptor.dir).and_then(|plan| {
                plan.find(&p.descriptor.requirement_id, &p.descriptor.check.id)
                    .cloned()
            }),
        };
        let Some(entry) = entry else { continue };

        let per_req = by_dir
            .entry(p.descriptor.dir.clone())
            .or_default()
            .entry(p.descriptor.requirement_id.clone())
            .or_insert_with(|| (p.descriptor.requirement_title.clone(), Vec::new()));
        per_req.1.push(entry);
    }

    by_dir
        .into_iter()
        .map(|(dir, reqs)| {
            let requirements = reqs
                .into_iter()
                .map(|(id, (title, checks))| plan_file::PlanRequirement { id, title, checks })
                .collect();
            (dir, plan_file::PlanFile::new(requirements))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The final record (MULTI-1829 code review: blocking items 3 and 4)
// ---------------------------------------------------------------------------

/// Build this run's one, authoritative [`presenter::FinalRecord`] directly
/// from `refused`/`processed`/`written_paths` — this run's own ground truth,
/// never from anything the live presenter did or didn't receive (see
/// [`presenter::FinalRecord`]'s docs on why). Both backends render from this
/// same value: [`print_plain_record`] prints it as plain text, and (via
/// `presenter::Presentation::final_record`) the inline TUI flushes it to
/// scrollback at teardown.
fn build_final_record(
    refused: &[RefusedCheck],
    processed: &[ProcessedCheck],
    written_paths: &[PathBuf],
    truncated_count: usize,
    total_checks: usize,
) -> presenter::FinalRecord {
    // Group by `req_index` (assigned by `partition_checks`/`assign_req_indices`),
    // in ascending order, refused and processed checks interleaved by that
    // shared key so a directory's refused and plannable requirements both
    // appear in the tree — exactly the live presenter tree's own grouping.
    let mut by_req: std::collections::BTreeMap<usize, (String, Vec<presenter::FinalCheck>)> =
        std::collections::BTreeMap::new();

    for r in refused {
        let entry = by_req
            .entry(r.req_index)
            .or_insert_with(|| (r.requirement_title.clone(), Vec::new()));
        entry.1.push(presenter::FinalCheck {
            title: r.check_title.clone(),
            outcome: presenter::FinalOutcome::Error {
                message: r.message.clone(),
            },
            stale_reason: None,
            truncated: false,
        });
    }
    for p in processed {
        let entry = by_req
            .entry(p.descriptor.req_index)
            .or_insert_with(|| (p.descriptor.requirement_title.clone(), Vec::new()));
        entry.1.push(final_check_for(p));
    }

    let requirements = by_req
        .into_values()
        .map(|(title, checks)| presenter::FinalRequirement { title, checks })
        .collect();

    presenter::FinalRecord {
        requirements,
        truncated_count,
        total_checks,
        written_paths: written_paths.to_vec(),
    }
}

/// One processed check's [`presenter::FinalCheck`]. The stale reason is
/// carried through for `Planned` **and** `AgentOnly` (both come from the same
/// `LineOutcome::Planned` — `decider` alone tells them apart) — the ticket:
/// "the stale reason for every re-planned check". A `Reused` (fresh) check
/// was never re-planned, so it never carries one; neither does an errored
/// attempt (`ProcessedCheck::stale_reason` is `Some` there too, but this
/// function deliberately doesn't surface it — the ticket only asks for it on
/// a check that was actually (re-)planned).
fn final_check_for(p: &ProcessedCheck) -> presenter::FinalCheck {
    let title = p.descriptor.check.title.clone();
    match &p.outcome {
        LineOutcome::Reused(entry) => presenter::FinalCheck {
            title,
            outcome: presenter::FinalOutcome::Fresh,
            stale_reason: None,
            truncated: calls_have_truncated(&entry.calls),
        },
        LineOutcome::Planned(planned) => {
            let truncated = calls_have_truncated(&planned.calls);
            let outcome = match planned.decider {
                Decider::Jev => presenter::FinalOutcome::Planned {
                    verdict: planned.verdict,
                },
                Decider::Agent(reason) => presenter::FinalOutcome::AgentOnly {
                    reason,
                    verdict: planned.verdict,
                },
            };
            presenter::FinalCheck {
                title,
                outcome,
                stale_reason: p.stale_reason,
                truncated,
            }
        }
        LineOutcome::Error(err) => presenter::FinalCheck {
            title,
            outcome: presenter::FinalOutcome::Error {
                message: err.to_string(),
            },
            stale_reason: None,
            truncated: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Output formatting
// ---------------------------------------------------------------------------

/// The plain final record printed to stdout when no TTY-owning live backend
/// already flushed it to scrollback (MULTI-1829) — the heartbeat backend, or
/// no live presenter at all (every orchestration test in this module).
/// Renders `record` — this run's one authoritative [`presenter::FinalRecord`]
/// — never anything reconstructed from live presenter state.
fn print_plain_record(terminal: &Terminal, record: &presenter::FinalRecord) -> Result<()> {
    for req in &record.requirements {
        terminal.write_stdout_line(&req.title)?;
        for check in &req.checks {
            terminal.write_stdout_line(&format!("  {}", final_check_line(check)))?;
        }
    }

    if let Some(line) = summary_line(record.truncated_count, record.total_checks) {
        terminal.write_stdout_line(&line)?;
    }

    if !record.written_paths.is_empty() {
        terminal.write_stdout_line("Wrote plan files:")?;
        for path in &record.written_paths {
            terminal.write_stdout_line(&format!("  {}", path.display()))?;
        }
    }

    Ok(())
}

/// One [`presenter::FinalCheck`]'s plain-text line for [`print_plain_record`]:
/// its title, terminal tag, truncated marker, and — for a re-planned check —
/// its stale reason (MULTI-1829 code review item 3).
fn final_check_line(check: &presenter::FinalCheck) -> String {
    let tag = match &check.outcome {
        presenter::FinalOutcome::Fresh => "fresh".to_string(),
        presenter::FinalOutcome::Planned { verdict } => {
            format!("planned (jev, {})", if *verdict { "pass" } else { "fail" })
        }
        presenter::FinalOutcome::AgentOnly { reason, verdict } => format!(
            "agent-only ({}, {})",
            agent_reason_str(*reason),
            if *verdict { "pass" } else { "fail" }
        ),
        presenter::FinalOutcome::Error { message } => format!("error: {message}"),
    };
    let mut line = format!("{}: {tag}", check.title);
    if check.truncated {
        line.push_str(" · truncated");
    }
    if let Some(reason) = check.stale_reason {
        line.push_str(&format!(" (stale: {})", reason.tag()));
    }
    line
}

/// The `N of M checks have truncated discovery` summary line, or `None` when
/// `truncated_count == 0` — the ticket: always shown "when N > 0", including
/// on a run that only reused entries (a truncated call is excluded from
/// freshness, so its owning entry is `reused`, not replanned, every time —
/// see the module docs).
fn summary_line(truncated_count: usize, total_checks: usize) -> Option<String> {
    (truncated_count > 0)
        .then(|| format!("{truncated_count} of {total_checks} checks have truncated discovery"))
}

/// The wire-string spelling of each [`AgentReason`], matching
/// [`plan_file::AgentReason`]'s own `#[serde(rename = "...")]` values exactly,
/// so a printed reason always matches what's written to `.check-plan.toml`.
///
/// `pub(crate)` (not private): also reused, verbatim, by `presenter`'s own
/// row-tag rendering (MULTI-1829) — one mapping from [`AgentReason`] to
/// display text, not two that could drift apart.
pub(crate) fn agent_reason_str(reason: AgentReason) -> &'static str {
    match reason {
        AgentReason::JevDisagreed => "jev disagreed",
        AgentReason::JevUncertain => "jev uncertain",
        AgentReason::ControlFailed => "control failed",
        AgentReason::OverBudget => "over budget",
        AgentReason::NoToolCalls => "no tool calls",
        AgentReason::TruncatedDiscovery => "truncated discovery",
    }
}

#[cfg(test)]
mod tests;
