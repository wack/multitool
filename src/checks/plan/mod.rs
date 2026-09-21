//! `multi plan` (MULTI-1824): establish what evidence is necessary to verify
//! each check and freeze it into `.check-plan.toml`.
//!
//! This is a **separate** subcommand and code path from `multi check` — see
//! `crate::checks`' own module docs. It reuses [`discovery::discover`]'s
//! walk/parse/validate function directly (not `DiscoveryActor`), so it gets
//! the exact same strict whole-run abort on an invalid `CHECKS.md`, and
//! reuses [`crate::checks::executor::CheckExecutor`]/
//! [`crate::checks::sandbox::Sandbox`] the same way `multi check` does — but
//! does **not** reuse the actor pipeline. [`Planner`] is this module's own DI
//! seam, and [`AgentPlanner`] is the real implementation — see `planner`'s
//! module docs for the calibration logic.
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
//! up by `(source, req_ordinal, check_ordinal, prompt_xxh64)` — see
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
//! One line per check (`planned` / `reused` / `agent-only <reason>` /
//! `error <message>`), followed by a `N of M checks have truncated
//! discovery` summary whenever `N > 0` — including on a run that only
//! reused entries, since a truncated call is excluded from freshness (MULTI-
//! 1822) and its owning entry is still `reused`, not replanned, every time.
//! Exit code `0` unless some check could not be planned (a refused file's
//! checks count as not-planned too).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use indexmap::IndexMap;
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

pub use planner::{AgentPlanner, PlanRequest, PlannedCheck, Planner};
// `BoxedPlanner`/`EntryContext` stay reachable only at `planner::{BoxedPlanner,
// EntryContext}` for now — no caller outside `planner.rs` itself needs them
// yet (`AgentPlanner` uses `Arc<dyn Planner>`, never `Box`, throughout this
// module), and re-exporting them here today would be an unused `pub use` in
// this privately-rooted module tree (`mod checks;` in `lib.rs` is not `pub`,
// so nothing outside the crate can ever reach a `pub` item here either) —
// widen this the moment something outside `planner.rs` needs one.
//
// `entry_from_outcome` IS re-exported flat despite having no caller in this
// ticket, because MULTI-1826's self-healing (`multi check` escalation) is
// documented to call it from outside this module (see its own docs on why
// it's the single entry-builder both callers share) — the same situation
// `crate::checks::config`'s `JevConfig`/`resolve_jev` re-export was in before
// MULTI-1824 became their first cross-module consumer.
#[allow(unused_imports)]
pub(crate) use planner::entry_from_outcome;
#[cfg(test)]
pub(crate) use planner::fake::FakePlanner;

/// Run `multi plan` rooted at `working_dir`. Builds the real [`AgentPlanner`]
/// (composing the same [`CheckExecutor`]/[`Sandbox`] `multi check` builds —
/// see [`config::Resolved::build_executor`]) and delegates to
/// [`run_with_planner`], the injectable core the tests drive directly with a
/// [`FakePlanner`].
///
/// Returns the process exit code (mirroring `crate::checks::run`'s own
/// contract): `0` unless a check could not be planned. An invalid suite (a
/// malformed `CHECKS.md`) or a Jev credential/request failure abort the run
/// as `Err` rather than reporting a numeric code.
pub async fn run(
    terminal: &Terminal,
    working_dir: &Path,
    overrides: CliOverrides,
    force: bool,
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

    let executor: Arc<dyn CheckExecutor + Send + Sync> = Arc::from(resolved.build_executor()?);
    let sandbox: Arc<dyn Sandbox + Send + Sync> = Arc::from(sandbox::select_sandbox());
    let jev_client = Arc::new(JevClient::from_config(&jev_config)?);
    let planner: Arc<dyn Planner + Send + Sync> = Arc::new(AgentPlanner::new(
        executor,
        sandbox,
        jev_client,
        jev_config,
        resolved.config.max_attempts,
    ));

    run_with_planner(
        terminal,
        &requirements,
        planner,
        resolved.config.concurrency,
        force,
    )
    .await
}

// ---------------------------------------------------------------------------
// Grouping: requirements -> one `.check-plan.toml` per directory
// ---------------------------------------------------------------------------

/// Every requirement declared in one directory's requirements file(s),
/// sharing one `.check-plan.toml` — see the module docs. Requirements within
/// a group are paired with `(source file name, ordinal within that source)`;
/// `root`/`root_source` are identical for every requirement in a group by
/// construction (both are resolved from the same directory — see
/// `discovery::repo_root::resolve`).
struct FileGroup<'a> {
    root: PathBuf,
    root_source: RootSource,
    requirements: Vec<(String, u32, &'a Requirement)>,
}

/// Group `requirements` by the directory containing their declaring file,
/// computing each requirement's `ordinal` as its 0-based position within its
/// own `(directory, source file name)` — i.e. within its declaring file,
/// since only `CHECKS.md` exists today (one source file per directory).
/// Insertion-ordered ([`IndexMap`]) so output/writing stays in discovery
/// order.
fn group_by_directory(requirements: &[Requirement]) -> IndexMap<PathBuf, FileGroup<'_>> {
    let mut groups: IndexMap<PathBuf, FileGroup<'_>> = IndexMap::new();
    let mut ordinals: HashMap<(PathBuf, String), u32> = HashMap::new();

    for req in requirements {
        let dir = req
            .filepath
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let source = req
            .filepath
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| req.filepath.display().to_string());

        let counter = ordinals.entry((dir.clone(), source.clone())).or_insert(0);
        let ordinal = *counter;
        *counter += 1;

        let group = groups.entry(dir.clone()).or_insert_with(|| FileGroup {
            root: req.root.clone(),
            root_source: req.root_source,
            requirements: Vec::new(),
        });
        group.requirements.push((source, ordinal, req));
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
    /// completes, and doubles as the [`crate::checks::model::CheckId`]
    /// `AgentRunRequest` wants for session-id namespacing.
    index: usize,
    dir: PathBuf,
    source: String,
    req_ordinal: u32,
    check_ordinal: u32,
    requirement_title: String,
    check: Check,
    root: PathBuf,
    declared_in: PathBuf,
}

/// One refused-file check: no descriptor was ever built for it (its file has
/// no manifest-derived root), but it still needs an `error` line and to count
/// toward the summary/exit code.
struct RefusedCheck {
    requirement_title: String,
    check_title: String,
    message: String,
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
    let mut descriptors = Vec::new();
    let mut refused = Vec::new();
    let mut valid_dirs = Vec::new();

    for (dir, group) in groups {
        if group.root_source == RootSource::ScanDirectory {
            for (_source, _req_ordinal, req) in &group.requirements {
                for check in &req.checks {
                    refused.push(RefusedCheck {
                        requirement_title: req.title.clone(),
                        check_title: check.title.clone(),
                        message: format!(
                            "no MultiTool.toml manifest found above `{}`; add one at the \
                             repository root to plan this file",
                            req.filepath.display(),
                        ),
                    });
                }
            }
            continue;
        }

        valid_dirs.push(dir.clone());
        for (source, req_ordinal, req) in &group.requirements {
            let declared_in =
                crate::checks::execution::declared_in_relative_to_root(&req.filepath, &group.root);
            for (check_ordinal, check) in req.checks.iter().enumerate() {
                let index = descriptors.len();
                descriptors.push(CheckDescriptor {
                    index,
                    dir: dir.clone(),
                    source: source.clone(),
                    req_ordinal: *req_ordinal,
                    check_ordinal: check_ordinal as u32,
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

// ---------------------------------------------------------------------------
// Caching: load existing plans, look up cached entries
// ---------------------------------------------------------------------------

/// Load the existing `.check-plan.toml` (if any) beside every directory in
/// `valid_dirs`, once each. Empty (no directory is ever loaded) when `force`
/// is set — every check is then (re)planned from scratch, since
/// [`cached_entry`] looks entries up in this map and finds nothing for an
/// absent key. A load failure (corrupt file, unknown version) aborts the
/// whole run — see the module docs.
fn load_existing_plans(
    valid_dirs: &[PathBuf],
    force: bool,
) -> Result<HashMap<PathBuf, plan_file::PlanFile>> {
    let mut out = HashMap::new();
    if force {
        return Ok(out);
    }
    for dir in valid_dirs {
        if let Some(loaded) = plan_file::PlanStore::load(dir)? {
            out.insert(dir.clone(), loaded);
        }
    }
    Ok(out)
}

/// The existing plan entry for `descriptor`, if `existing` has a plan for its
/// directory and that plan has a matching, hash-valid entry.
fn cached_entry(
    descriptor: &CheckDescriptor,
    existing: &HashMap<PathBuf, plan_file::PlanFile>,
) -> Option<plan_file::PlanCheck> {
    let plan = existing.get(&descriptor.dir)?;
    let hash = plan_file::prompt_xxh64(&descriptor.check.title, &descriptor.check.prompt);
    plan.lookup(
        &descriptor.source,
        descriptor.req_ordinal,
        descriptor.check_ordinal,
        &hash,
    )
    .cloned()
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
}

/// Resolve one check: replay its cached entry (if any) and reuse it when
/// still [`Freshness::Fresh`]; otherwise call the planner. `cached` is
/// already `None` for a `--force` run (see [`load_existing_plans`]), so this
/// function doesn't need its own force-awareness.
async fn process_check(
    descriptor: CheckDescriptor,
    cached: Option<plan_file::PlanCheck>,
    planner: Arc<dyn Planner + Send + Sync>,
) -> ProcessedCheck {
    if let Some(entry) = cached {
        let replayed =
            replay::replay_check(&descriptor.check.title, &entry.calls, &descriptor.root).await;
        if replayed.freshness == Freshness::Fresh {
            return ProcessedCheck {
                outcome: LineOutcome::Reused(entry),
                descriptor,
            };
        }
    }

    let req = PlanRequest {
        check_id: descriptor.index,
        check: descriptor.check.clone(),
        requirement_title: descriptor.requirement_title.clone(),
        declared_in: descriptor.declared_in.clone(),
        root: descriptor.root.clone(),
    };
    let outcome = match planner.plan_check(req).await {
        Ok(planned) => LineOutcome::Planned(planned),
        Err(err) => LineOutcome::Error(err),
    };
    ProcessedCheck {
        descriptor,
        outcome,
    }
}

/// The injectable core of `multi plan`, driven directly by tests with a
/// [`FakePlanner`] (or a real [`AgentPlanner`] over a `FakeExecutor` + mock
/// Jev). See the module docs for the full contract.
pub(crate) async fn run_with_planner(
    terminal: &Terminal,
    requirements: &[Requirement],
    planner: Arc<dyn Planner + Send + Sync>,
    concurrency: usize,
    force: bool,
) -> Result<i32> {
    if requirements.is_empty() {
        terminal.write_stdout_line("No requirements found.")?;
        return Ok(0);
    }

    let groups = group_by_directory(requirements);
    let (descriptors, refused, valid_dirs) = partition_checks(&groups);
    let existing = load_existing_plans(&valid_dirs, force)?;

    let cached: Vec<Option<plan_file::PlanCheck>> = descriptors
        .iter()
        .map(|d| cached_entry(d, &existing))
        .collect();

    let processed: Vec<ProcessedCheck> = stream::iter(descriptors.into_iter().zip(cached))
        .map(|(descriptor, cached)| {
            let planner = Arc::clone(&planner);
            async move { process_check(descriptor, cached, planner).await }
        })
        .buffer_unordered(concurrency.max(1))
        .collect()
        .await;

    // Abort the whole run on the first `AbortPlanRun`-marked failure — a bad
    // or missing Jev credential must not silently read as every check
    // disagreeing with the agent. No plan file is written in this case.
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

    // -- output: refused files first, then every processed check ----------
    for refused_check in &refused {
        terminal.write_stdout_line(&format_error_line(
            &refused_check.requirement_title,
            &refused_check.check_title,
            &refused_check.message,
        ))?;
    }
    for p in &processed {
        terminal.write_stdout_line(&line_for(p))?;
    }

    let truncated_count = processed
        .iter()
        .filter(|p| entry_has_truncated_call(&p.outcome))
        .count();
    let total_checks = refused.len() + processed.len();
    if truncated_count > 0 {
        terminal.write_stdout_line(&format!(
            "{truncated_count} of {total_checks} checks have truncated discovery"
        ))?;
    }

    let has_error = !refused.is_empty()
        || processed
            .iter()
            .any(|p| matches!(p.outcome, LineOutcome::Error(_)));

    // -- write: one `.check-plan.toml` per directory, exactly once --------
    let mut files = build_plan_files(&processed);
    for dir in &valid_dirs {
        files
            .entry(dir.clone())
            .or_insert_with(|| plan_file::PlanFile::new(vec![]));
    }
    for (dir, file) in &files {
        plan_file::PlanStore::write(dir, file)?;
    }

    Ok(if has_error { 1 } else { 0 })
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
/// One directory's requirements under construction, keyed by
/// `(source, req_ordinal)` — see [`build_plan_files`].
type DirectoryRequirements = HashMap<(String, u32), RequirementChecks>;

/// Assemble one [`plan_file::PlanFile`] per directory from `processed`'s
/// surviving entries (an errored check contributes no entry — dropped, the
/// same as a check no longer present in `CHECKS.md`). [`plan_file::PlanStore::write`]
/// re-sorts requirements/checks deterministically on its own, so insertion
/// order here doesn't matter.
fn build_plan_files(processed: &[ProcessedCheck]) -> HashMap<PathBuf, plan_file::PlanFile> {
    let mut by_dir: HashMap<PathBuf, DirectoryRequirements> = HashMap::new();

    for p in processed {
        let entry = match &p.outcome {
            LineOutcome::Reused(entry) => Some(entry.clone()),
            LineOutcome::Planned(planned) => {
                Some(planned.clone().into_plan_check(p.descriptor.check_ordinal))
            }
            LineOutcome::Error(_) => None,
        };
        let Some(entry) = entry else { continue };

        let per_req = by_dir
            .entry(p.descriptor.dir.clone())
            .or_default()
            .entry((p.descriptor.source.clone(), p.descriptor.req_ordinal))
            .or_insert_with(|| (p.descriptor.requirement_title.clone(), Vec::new()));
        per_req.1.push(entry);
    }

    by_dir
        .into_iter()
        .map(|(dir, reqs)| {
            let requirements = reqs
                .into_iter()
                .map(
                    |((source, ordinal), (title, checks))| plan_file::PlanRequirement {
                        title,
                        source,
                        ordinal,
                        checks,
                    },
                )
                .collect();
            (dir, plan_file::PlanFile::new(requirements))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Output formatting
// ---------------------------------------------------------------------------

fn line_for(p: &ProcessedCheck) -> String {
    let requirement_title = &p.descriptor.requirement_title;
    let check_title = &p.descriptor.check.title;
    match &p.outcome {
        LineOutcome::Reused(_) => format!("reused     {requirement_title} :: {check_title}"),
        LineOutcome::Planned(planned) => match planned.decider {
            Decider::Jev => format!("planned    {requirement_title} :: {check_title}"),
            Decider::Agent(reason) => format!(
                "agent-only {requirement_title} :: {check_title} (reason: {})",
                agent_reason_str(reason)
            ),
        },
        LineOutcome::Error(err) => {
            format_error_line(requirement_title, check_title, &err.to_string())
        }
    }
}

fn format_error_line(requirement_title: &str, check_title: &str, message: &str) -> String {
    format!("error      {requirement_title} :: {check_title}: {message}")
}

/// The wire-string spelling of each [`AgentReason`], matching
/// [`plan_file::AgentReason`]'s own `#[serde(rename = "...")]` values exactly,
/// so a printed reason always matches what's written to `.check-plan.toml`.
fn agent_reason_str(reason: AgentReason) -> &'static str {
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
