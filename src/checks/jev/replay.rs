//! In-host replay of a plan's frozen `Read`/`Grep`/`Glob` calls, and
//! freshness classification (MULTI-1822).
//!
//! Both `multi plan` (cache validation — MULTI-1824) and `multi check`
//! (evidence gathering — MULTI-1825) need to re-run a plan's frozen calls
//! against the live working tree, checksum the results, and compare against
//! stored checksums. This module is the one place that happens.
//!
//! ## Why replay invokes the real cersei tools instead of re-implementing them
//!
//! [`replay_call`] hand-builds a [`ToolContext`] (`working_dir` = the
//! repository root, `permissions` = [`AllowReadOnly`]) and invokes the exact
//! same [`Jailed`]-wrapped `FileReadTool`/`GrepTool`/`GlobTool` that
//! [`super::super::executor::cersei::read_only_tools`] grants a check's
//! agent — no sandbox, no agent, no model call. Reusing the tools guarantees
//! the output *format* matches what the reasoning agent saw, and — because
//! replay calls `Tool::execute` directly rather than going through
//! `cersei_agent::Agent::run` — it sees the tool's full, **uncapped** output.
//! Tracing the pinned `cersei` source
//! (`e75278d2d22d67674c154512de6d43c4161139bf`) confirms there *is* a second,
//! agent-facing cap the reasoning agent's context is subject to but replay
//! never is: `cersei-agent/src/runner.rs`'s `cap_tool_result` rewrites a long
//! result to a head/tail excerpt (`"[... N lines omitted ..."`) before
//! inserting it into the conversation, unconditionally, on top of each
//! tool's own cap (see below). A second layer, `compress_tool_output_with_stats`,
//! is gated behind `Agent::compression_level` (default `Off`, and
//! `CerseiExecutor` never raises it) — a no-op today, but replay bypasses it
//! either way by never routing through the runner at all. This is exactly
//! the asymmetry the ticket calls out: replay must see the uncapped text,
//! and it does, simply by construction.
//!
//! ## Re-absolutizing stored paths
//!
//! A `.check-plan.toml` call's `input` is root-relative (see
//! [`super::plan_file::relativize`]). [`reabsolutize`] turns it back into an
//! absolute path under `root` before invoking the tool — never relying on
//! [`cersei_tools::grep_tool::GrepTool`]/[`cersei_tools::glob_tool::GlobTool`]'s
//! own "default to `ctx.working_dir` when `path` is omitted" behavior, because
//! [`super::plan_file::relativize`] always writes an *explicit* `path` (`"."`
//! when the call omitted one), and a literal `"."` string handed to the tool
//! would be resolved against the *process* cwd, not `ctx.working_dir` — see
//! `GrepTool::execute`'s `search_path` construction. Passing an absolute path
//! sidesteps that entirely, which is also how this module honors the
//! contract that replay never depends on the process's current directory (see
//! the `replay_never_depends_on_process_cwd` test).
//!
//! A stored path that resolves outside `root` (a hand-edited or corrupted
//! plan) is *not* rejected by [`reabsolutize`] itself — [`build_tool`] wraps
//! every tool in the same [`Jailed`] decorator the real executor uses, which
//! rejects it at call time. Reusing that machinery, rather than duplicating
//! its boundary check here, is deliberate: one jail, one place it can have a
//! bug.
//!
//! ## Output normalization
//!
//! [`normalize_output`] strips `root`'s prefix from `Grep`/`Glob` output, in
//! every spelling `root` might arrive in or be echoed back in: as given,
//! canonicalized, and macOS's `/var` <-> `/private/var` (and `/tmp`, `/etc`)
//! symlink aliasing. This is what makes output — and therefore checksums —
//! machine-independent: two absolute roots over identical trees normalize to
//! identical text (see the
//! `identical_trees_at_different_roots_produce_identical_checksums` test).
//!
//! This is deliberately **tool-aware and line-prefix-only**, never a global
//! substring replace over a tool's whole output: `Read`'s output is file
//! *content*, not paths (cersei's `FileReadTool` never prints one — see
//! [`normalize_output`]'s docs), so it passes through completely untouched;
//! `Grep`/`Glob` strip a matching root spelling *only* when it is the exact
//! leading prefix of a line (a `Glob` line is nothing but a path; a `Grep`
//! line's path is always its own prefix, per `grep_tool.rs`'s
//! `format!("{}:{}:{}", m.file.display(), ...)`). A global replace was this
//! module's original design and its bug: it also rewrote root-shaped text
//! *inside* file content and matched Grep *content*, which could silently
//! leave a checksum unchanged for evidence whose meaning actually changed
//! (a false `Fresh`) and silently altered the evidence text `multi check`
//! would forward to Jev (MULTI-1823).
//!
//! ## Checksums
//!
//! Checksums are over what a call *means*, not its raw bytes — see
//! [`compute_checksums`]. `Read`/`Glob` hash the normalized output directly.
//! `Grep` hashes two things separately: `files_xxh64` (the sorted, deduped
//! set of matched root-relative paths — the *discovery* half) and
//! `lines_xxh64` (every `path:content` pair with line numbers stripped and
//! then sorted — the *content* half), so a line-number-only shift (an
//! insertion above a match) leaves both unchanged. Parsing a `path:line:content`
//! line splits on the *first* `:<digits>:` after the root prefix is already
//! stripped — see [`split_grep_line`] — which is exactly what the ticket
//! specifies and is robust to colons in `content` (the true separator, being
//! the line's own prefix, is always found first in a left-to-right scan; a
//! matched *path* containing a `:<digits>:`-shaped substring is a known,
//! unaddressed limitation — see that function's docs).
//!
//! `GrepTool` has exactly one output shape today — verified against the
//! pinned source, not assumed (see [`GREP_KNOWN_INPUT_KEYS`]) — but
//! [`compute_checksums`] still refuses to trust that shape for an `input`
//! carrying any key outside that verified allowlist, falling back to an
//! opaque whole-output checksum where *any* change reads as
//! [`Freshness::DiscoveryStale`] rather than risk a wrong-but-stable
//! `files_xxh64` for a format it doesn't actually understand.
//!
//! `Glob`'s own result ordering was verified too: the pinned
//! `tool_primitives::search::glob_blocking` sorts its paths
//! (`paths.sort()`, lexicographic on `PathBuf`, not modification time or
//! walk/creation order) *before* truncating, so a `Glob` checksum is already
//! independent of file creation order or mtime for any non-truncated result
//! — confirmed by the
//! `glob_checksum_is_independent_of_file_creation_order_and_mtime` test
//! rather than only asserted.
//!
//! ## Classification safety
//!
//! [`classify_call`] never reports `Fresh`/`ReadsStale` when the truth might
//! be `DiscoveryStale` — the dangerous direction, because it lets a cheaper
//! decider be trusted wrongly. Concretely: a [`ReplayedCall::missing`]
//! result always wins ([`Freshness::DiscoveryStale`]); a truncation-status
//! *mismatch* between `stored` and `replayed` (either direction) is itself
//! evidence discovery changed, so it classifies as `DiscoveryStale` rather
//! than being silently excluded (excluding it unconditionally — this
//! module's original bug — let a `Grep`/`Glob` that newly grew past its cap
//! aggregate to `Fresh`); only a truncation match on *both* sides excludes a
//! call from freshness entirely; and a `stored`/`replayed` tool mismatch
//! (unreachable through [`replay_check`], but not through a direct
//! [`classify_call`] caller) also reads as `DiscoveryStale`, so a
//! coincidental [`CallChecksum::Single`] collision between `Read` and `Glob`
//! can never read as `Fresh`.
//!
//! ## Truncation and windowing
//!
//! `Grep`'s 250-match cap (`grep_tool.rs`: `max_results: Some(250)`,
//! hardcoded — no tool-input field overrides it) gives no positive signal
//! when hit, so [`is_truncated_grep`] conservatively treats *exactly* 250
//! result lines as truncated — a known, accepted false positive when a
//! search genuinely has exactly 250 matches and no more (see the ticket's
//! "known, serious limitation" language; a real fix is a configurable cap in
//! `wack/cersei`, out of scope here and for this repo per the One Repo Rule).
//! `Glob`'s cap is only a *default* of 200 (`glob_tool.rs`:
//! `input.limit.unwrap_or(200)`) — a call may override it via its own
//! `limit` input — so [`is_truncated_glob`] detects truncation from the
//! tool's own notice text (which embeds whatever limit was actually in
//! effect) rather than a hardcoded 200. Both numbers were verified against
//! the pinned source rather than trusted from the ticket text, which
//! describes Glob's 200 as fixed; it is a default.
//!
//! `Grep`'s cap has no interaction with any per-call override, because
//! `GrepTool`'s `Input` struct (`grep_tool.rs`) has no `head_limit` (or any
//! other result-count) field at all — `max_results: Some(250)` is passed to
//! `tool_primitives::search::grep` unconditionally, verified against the
//! pinned source, not assumed. See [`GREP_KNOWN_INPUT_KEYS`] for the related
//! finding that `GrepTool` also has no alternate *output* mode a call could
//! select.
//!
//! A `Read` is windowed when its stored input carries `offset`/`limit`, or —
//! for a call that omitted both — when its output happens to hit exactly the
//! tool's 2000-line default (`file_read.rs`: `input.limit.unwrap_or(2000)`).
//! The latter is a coincidental false positive when a file has *exactly*
//! 2000 lines and no more; accepted for the same reason as Grep's cap.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cersei_tools::permissions::AllowReadOnly;
use cersei_tools::{CostTracker, Extensions, Tool, ToolContext, ToolResult};
use serde_json::Value;

use crate::checks::executor::ReadOnlyTool;
use crate::checks::executor::jail::Jailed;

use super::plan_file::{PlanCall, xxh64_hex};

// ---------------------------------------------------------------------------
// Constants (verified against the pinned cersei source — see module docs).
// ---------------------------------------------------------------------------

/// cersei's `GrepTool` hardcoded match cap (`grep_tool.rs`:
/// `max_results: Some(250)`). See the module docs' "Truncation and windowing"
/// section for why hitting it exactly is treated as truncated.
const GREP_MATCH_CAP: usize = 250;

/// cersei's `FileReadTool` default line limit when a call omits `limit`
/// (`file_read.rs`: `input.limit.unwrap_or(2000)`).
const READ_DEFAULT_LIMIT: usize = 2000;

/// The exact text `GrepTool` returns for zero matches (`grep_tool.rs`:
/// `ToolResult::success("No matches found.")`) — the empty file/line set.
const GREP_NO_MATCHES: &str = "No matches found.";

/// A substring of `GlobTool`'s truncation notice (`glob_tool.rs`:
/// `"\n\n[Showing the first {limit} matches; more exist. Use a more specific \
/// pattern to narrow results.]"`), stable regardless of the call's effective
/// `limit` (which the `{limit}` interpolation embeds).
const GLOB_TRUNCATION_NOTICE: &str = "more exist. Use a more specific pattern to narrow results.";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// How stale a check's frozen evidence is, from a replay — the most severe
/// class among its calls. Ordered `Fresh < ReadsStale < DiscoveryStale` (see
/// the `#[derive(Ord)]`, which follows declaration order) so aggregating a
/// check's calls is a plain `max()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Freshness {
    /// Every non-truncated call reproduced its stored checksum(s) exactly.
    Fresh,
    /// Some call's *content* changed, but not the set of files it depends on
    /// (a `Grep`'s matched text, or a whole-file `Read`'s content).
    ReadsStale,
    /// Some call's *discovery* changed — the set of files it depends on is no
    /// longer what the plan assumed (a `Glob`, a `Grep`'s matched file set, a
    /// windowed `Read` whose file changed, or any call whose replay reported
    /// [`ReplayedCall::missing`]).
    DiscoveryStale,
}

/// The checksum(s) computed for one replayed call — the same shape
/// [`PlanCall`] stores, kept separate so replay can hand it to either
/// consumer: [`classify_call`] (compare against a stored `PlanCall`) or
/// [`ReplayedCall::to_plan_call`] (freeze a fresh one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallChecksum {
    /// `Read`/`Glob`: one `xxh64` over the normalized output.
    Single(String),
    /// `Grep`: the discovery half (sorted, deduped matched paths) and the
    /// content half (`path:content` pairs, line numbers stripped, sorted).
    Grep {
        files_xxh64: String,
        lines_xxh64: String,
    },
}

/// One call's replay result.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayedCall {
    pub tool: ReadOnlyTool,
    /// The call's input exactly as stored in the plan (root-relative) — not
    /// the re-absolutized form actually sent to the tool.
    pub input: Value,
    /// The tool's root-normalized output. `None` when [`Self::missing`]
    /// (the tool call itself errored — nothing meaningful to show).
    pub output: Option<String>,
    /// `None` when [`Self::truncated`] or [`Self::missing`] — neither has a
    /// reproducible result to checksum.
    pub checksums: Option<CallChecksum>,
    /// Only ever `true` for `Read`: the call's input carried `offset`/`limit`,
    /// or its output happened to hit the tool's 2000-line default. See the
    /// module docs.
    pub windowed: bool,
    /// The call hit its tool's result cap (`Grep`: exactly 250 lines;
    /// `Glob`: the truncation notice) — not reproducible, excluded from
    /// freshness comparison, no checksum.
    pub truncated: bool,
    /// The replay itself errored (e.g. the stored path no longer exists, or
    /// — for a corrupted plan — was rejected by the jail for escaping
    /// `root`). Always classifies as [`Freshness::DiscoveryStale`].
    pub missing: bool,
}

impl ReplayedCall {
    /// Build the `PlanCall` a plan would freeze for this replay — the
    /// "compute" side of replay's two uses (MULTI-1824 freezes a freshly
    /// captured agent call this way; [`classify_call`] is the "compare"
    /// side, used both by `multi plan`'s incremental re-planning and by
    /// `multi check`'s evidence-freshness check). `None` for a truncated or
    /// missing call — neither has a checksum to freeze; a caller that wants
    /// to freeze a truncated call builds [`PlanCall::Truncated`] directly
    /// from `self.tool`/`self.input`, which needs nothing this method
    /// computes.
    pub fn to_plan_call(&self) -> Option<PlanCall> {
        let checksums = self.checksums.clone()?;
        Some(match checksums {
            CallChecksum::Single(xxh64) => match self.tool {
                ReadOnlyTool::Read => PlanCall::Read {
                    input: self.input.clone(),
                    xxh64,
                },
                ReadOnlyTool::Glob => PlanCall::Glob {
                    input: self.input.clone(),
                    xxh64,
                },
                ReadOnlyTool::Grep => {
                    unreachable!("compute_checksums only ever pairs Grep with CallChecksum::Grep")
                }
            },
            CallChecksum::Grep {
                files_xxh64,
                lines_xxh64,
            } => PlanCall::Grep {
                input: self.input.clone(),
                files_xxh64,
                lines_xxh64,
            },
        })
    }
}

/// A check's replay result: the aggregated [`Freshness`] plus every call's
/// individual result.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayedCheck {
    pub freshness: Freshness,
    pub calls: Vec<ReplayedCall>,
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// Replay one stored call: re-absolutize its input under `root`, invoke the
/// real, jailed cersei tool, normalize the output, and compute checksums.
/// Pure IO plus deterministic post-processing — no comparison against any
/// stored value happens here (see [`classify_call`] for that).
///
/// Tools are async cersei `Tool`s that may do blocking IO internally (both
/// `Read` and the ripgrep-powered `Grep`/`Glob` primitives use
/// `tokio::task::spawn_blocking` — see `tool_primitives::fs`/`search` in the
/// pinned cersei source), so this is safe to call from the tokio runtime the
/// rest of the pipeline already runs on.
pub async fn replay_call(tool: ReadOnlyTool, input: &Value, root: &Path) -> ReplayedCall {
    let absolutized = reabsolutize(tool, input, root);
    let ctx = tool_context(root);
    let result: ToolResult = build_tool(tool).execute(absolutized, &ctx).await;

    if result.is_error {
        return ReplayedCall {
            tool,
            input: input.clone(),
            output: None,
            checksums: None,
            windowed: false,
            truncated: false,
            missing: true,
        };
    }

    let normalized = normalize_output(tool, &result.content, root);

    let (truncated, windowed) = match tool {
        ReadOnlyTool::Read => (false, is_windowed_read(input, &normalized)),
        ReadOnlyTool::Grep => (is_truncated_grep(&normalized), false),
        ReadOnlyTool::Glob => (is_truncated_glob(&normalized), false),
    };

    if truncated {
        return ReplayedCall {
            tool,
            input: input.clone(),
            output: Some(normalized),
            checksums: None,
            windowed: false,
            truncated: true,
            missing: false,
        };
    }

    ReplayedCall {
        tool,
        input: input.clone(),
        checksums: Some(compute_checksums(tool, input, &normalized)),
        output: Some(normalized),
        windowed,
        truncated: false,
        missing: false,
    }
}

/// Replay every call a plan froze for one check, classify each against its
/// stored checksum(s), and aggregate the check's [`Freshness`] as the most
/// severe class among its non-truncated calls (`Fresh` when there are none —
/// vacuously true, matching a check with zero calls).
///
/// `check_title` identifies the owning check in the warn log emitted for
/// every truncated-call detection (exactly one per detection, naming the
/// check, the tool, and the call's input — see [`is_truncated_grep`]/
/// [`is_truncated_glob`] for what "truncated" means per tool).
pub async fn replay_check(check_title: &str, calls: &[PlanCall], root: &Path) -> ReplayedCheck {
    let mut replayed = Vec::with_capacity(calls.len());
    let mut worst = Freshness::Fresh;

    for stored in calls {
        let (tool, input) = call_identity(stored);
        let call = replay_call(tool, input, root).await;

        if call.truncated {
            tracing::warn!(
                check = check_title,
                tool = ?tool,
                input = %input,
                "replay hit the tool's result cap; excluding this call from freshness comparison",
            );
        }

        if let Some(freshness) = classify_call(stored, &call) {
            worst = worst.max(freshness);
        }
        replayed.push(call);
    }

    ReplayedCheck {
        freshness: worst,
        calls: replayed,
    }
}

/// Extract a stored call's `(tool, input)` identity — every [`PlanCall`]
/// variant carries both, just shaped differently per the checksums it holds.
fn call_identity(call: &PlanCall) -> (ReadOnlyTool, &Value) {
    match call {
        PlanCall::Read { input, .. } => (ReadOnlyTool::Read, input),
        PlanCall::Glob { input, .. } => (ReadOnlyTool::Glob, input),
        PlanCall::Grep { input, .. } => (ReadOnlyTool::Grep, input),
        PlanCall::Truncated { tool, input } => (*tool, input),
    }
}

// ---------------------------------------------------------------------------
// Classification (pure — no IO)
// ---------------------------------------------------------------------------

/// Classify one call's freshness by comparing `stored`'s checksum(s) against
/// a fresh `replayed` result. `None` means "excluded from freshness" — this
/// happens *only* when the call was truncated both when it was originally
/// captured ([`PlanCall::Truncated`]) *and* on this replay
/// ([`ReplayedCall::truncated`]): there is genuinely nothing comparable on
/// either side. Every other combination returns `Some`, per this module's
/// guiding principle — understating staleness (reporting `Fresh`/
/// `ReadsStale` when the truth is `DiscoveryStale`) is the dangerous
/// direction, because it lets a cheaper decider be trusted wrongly:
///
/// * A [`ReplayedCall::missing`] result always classifies as
///   [`Freshness::DiscoveryStale`], regardless of what `stored` says —
///   checked first, before anything about truncation.
/// * A truncation-status **mismatch** — the call was truncated before but
///   replays cleanly now, or replayed cleanly before but is truncated now —
///   is itself evidence discovery changed (the cap boundary moved), so it
///   classifies as [`Freshness::DiscoveryStale`] rather than being excluded.
///   Excluding *any* truncated call unconditionally (this module's original
///   bug) let a newly-over-the-cap `Grep`/`Glob` — discovery that grew
///   without bound — silently aggregate to `Fresh`.
/// * A `stored`/`replayed` pairing whose tool doesn't match (only reachable
///   by a caller that mispairs them; [`replay_check`] never does) also
///   classifies as [`Freshness::DiscoveryStale`], so a coincidental
///   [`CallChecksum::Single`] collision between `Read` and `Glob` can never
///   read as `Fresh`.
///
/// This is the "compare" half of replay's two uses (see
/// [`ReplayedCall::to_plan_call`] for the "compute" half) — pure and
/// synchronous, so `multi plan`'s incremental re-planning and `multi
/// check`'s evidence-freshness check can both call it without re-running any
/// IO themselves.
pub fn classify_call(stored: &PlanCall, replayed: &ReplayedCall) -> Option<Freshness> {
    if replayed.missing {
        return Some(Freshness::DiscoveryStale);
    }

    let stored_truncated = matches!(stored, PlanCall::Truncated { .. });
    match (stored_truncated, replayed.truncated) {
        (true, true) => return None,
        (true, false) | (false, true) => return Some(Freshness::DiscoveryStale),
        (false, false) => {}
    }

    if call_identity(stored).0 != replayed.tool {
        return Some(Freshness::DiscoveryStale);
    }

    match (stored, replayed.checksums.as_ref()) {
        (PlanCall::Read { xxh64: old, .. }, Some(CallChecksum::Single(new))) => {
            Some(if old == new {
                Freshness::Fresh
            } else if replayed.windowed {
                Freshness::DiscoveryStale
            } else {
                Freshness::ReadsStale
            })
        }
        (PlanCall::Glob { xxh64: old, .. }, Some(CallChecksum::Single(new))) => {
            Some(if old == new {
                Freshness::Fresh
            } else {
                Freshness::DiscoveryStale
            })
        }
        (
            PlanCall::Grep {
                files_xxh64: old_files,
                lines_xxh64: old_lines,
                ..
            },
            Some(CallChecksum::Grep {
                files_xxh64: new_files,
                lines_xxh64: new_lines,
            }),
        ) => Some(if old_files != new_files {
            Freshness::DiscoveryStale
        } else if old_lines != new_lines {
            Freshness::ReadsStale
        } else {
            Freshness::Fresh
        }),
        // Truly defensive at this point: the explicit tool check above
        // already rejects any `stored`/`replayed` tool mismatch, and
        // `compute_checksums` only ever pairs `Read`/`Glob` with `Single`
        // and `Grep` with `CallChecksum::Grep`, so every remaining
        // combination here is unreachable in practice. Kept rather than
        // `unreachable!()` so a future shape this reasoning misses fails
        // safe (`DiscoveryStale`) instead of panicking.
        _ => Some(Freshness::DiscoveryStale),
    }
}

// ---------------------------------------------------------------------------
// Tool invocation
// ---------------------------------------------------------------------------

/// A minimal [`ToolContext`] for replay: `working_dir` is the repository
/// root (never the process cwd — replay always passes an absolute,
/// re-absolutized path anyway, but the jail also reads `working_dir` as its
/// boundary), permissions are [`AllowReadOnly`] (defense in depth: replay
/// only ever builds read-only tools, but the real executor's own policy is
/// mirrored here rather than omitted), and no session/cost/MCP state is
/// needed since replay is not itself an agent turn.
fn tool_context(root: &Path) -> ToolContext {
    ToolContext {
        working_dir: root.to_path_buf(),
        session_id: "jev-replay".to_string(),
        permissions: Arc::new(AllowReadOnly),
        cost_tracker: Arc::new(CostTracker::new()),
        mcp_manager: None,
        extensions: Extensions::default(),
    }
}

/// Build the one [`Jailed`]-wrapped cersei tool `tool` names — the same
/// construction as [`super::super::executor::cersei::read_only_tools`], one
/// tool at a time (replay only ever needs the single tool a frozen call
/// named). Kept in lockstep with `read_only_tools` by the
/// `build_tool_matches_the_wire_name_for_every_read_only_tool` test below;
/// `read_only_tools` itself is private to `executor::cersei` and not reused
/// directly, since it always builds all three.
fn build_tool(tool: ReadOnlyTool) -> Box<dyn Tool> {
    match tool {
        ReadOnlyTool::Read => Box::new(Jailed::path_keys(
            cersei_tools::file_read::FileReadTool,
            &["file_path"],
        )),
        ReadOnlyTool::Grep => Box::new(Jailed::path_keys(
            cersei_tools::grep_tool::GrepTool,
            &["path"],
        )),
        ReadOnlyTool::Glob => Box::new(Jailed::glob(
            cersei_tools::glob_tool::GlobTool,
            &["path"],
            "pattern",
        )),
    }
}

/// Turn a stored, root-relative call `input` back into an absolute one — the
/// inverse of [`super::plan_file::relativize`], applied fresh at replay time.
/// Infallible: a path that resolves outside `root` is *not* rejected here —
/// [`build_tool`]'s [`Jailed`] wrapper rejects it at call time (see the
/// module docs).
fn reabsolutize(tool: ReadOnlyTool, input: &Value, root: &Path) -> Value {
    let path_key = match tool {
        ReadOnlyTool::Read => "file_path",
        ReadOnlyTool::Grep | ReadOnlyTool::Glob => "path",
    };

    let mut input = input.clone();
    if let Some(obj) = input.as_object_mut()
        && let Some(Value::String(relative)) = obj.get(path_key).cloned()
    {
        let absolute = if relative == "." {
            root.to_path_buf()
        } else {
            root.join(&relative)
        };
        obj.insert(
            path_key.to_string(),
            Value::String(absolute.to_string_lossy().into_owned()),
        );
    }
    input
}

// ---------------------------------------------------------------------------
// Output normalization
// ---------------------------------------------------------------------------

/// Strip `root`'s prefix from `content` so replay output — and the
/// checksums computed over it — are machine-independent. **Tool-aware and
/// deliberately narrow**: only an absolute *path* a tool itself printed is
/// ever touched, never a tool's *content*.
///
/// * `Read`'s output is file content only — cersei's `FileReadTool` returns
///   `fc.content` alone, never `fc.path` (`file_read.rs`:
///   `ToolResult::success(fc.content)`; the `path` field on
///   `tool_primitives::fs::FileContent` is computed but never surfaced to the
///   tool's caller) — so there is no path to strip, and `Read`'s content
///   passes through completely untouched. Globally replacing a root-shaped
///   substring anywhere in a file's own content (this module's original bug)
///   would silently rewrite a source line that happens to mention an
///   absolute path matching `root`, producing an unchanged checksum for a
///   file that actually changed — a false `Fresh`.
/// * `Glob` output is one absolute path per line (plus, when truncated, a
///   trailing notice that never looks like a path). Each line is stripped
///   *only* if `root` (in some spelling) is an exact prefix of that whole
///   line — never a substring search.
/// * `Grep` output is `path:line:content` per line, with `path` always the
///   line's own leading text (`grep_tool.rs`:
///   `format!("{}:{}:{}", m.file.display(), ...)`). Only that leading prefix
///   is stripped, exactly as for `Glob`; `content` — which never begins at
///   the start of the line — is never touched, so a match's content
///   containing root-shaped text (e.g. a comment naming an absolute path)
///   survives normalization unchanged and still checksums as itself.
///
/// Every spelling `root` could plausibly appear as is tried: raw,
/// canonicalized, and the macOS `/var`/`/tmp`/`/etc` <-> `/private/...`
/// alias of each (see [`macos_spellings`]/[`root_spellings`]), longest first
/// so a `/private`-prefixed spelling is tried before its shorter alias.
fn normalize_output(tool: ReadOnlyTool, content: &str, root: &Path) -> String {
    match tool {
        ReadOnlyTool::Read => content.to_string(),
        ReadOnlyTool::Grep | ReadOnlyTool::Glob => {
            let candidates = root_spellings(root);
            content
                .lines()
                .map(|line| strip_leading_root(line, &candidates))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }
}

/// Every spelling of `root` that could appear as a leading path prefix in
/// `Grep`/`Glob` output: raw, canonicalized, and the macOS
/// `/var`/`/tmp`/`/etc` <-> `/private/...` alias of each (see
/// [`macos_spellings`]). Sorted longest-first so a `/private`-prefixed
/// spelling is tried before its shorter alias could match a truncated
/// prefix of it.
fn root_spellings(root: &Path) -> Vec<PathBuf> {
    let mut candidates = macos_spellings(root);
    if let Ok(canonical) = root.canonicalize() {
        for spelling in macos_spellings(&canonical) {
            if !candidates.contains(&spelling) {
                candidates.push(spelling);
            }
        }
    }
    candidates.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
    candidates
}

/// Strip a leading `<candidate>/` prefix from `line` (the first `candidates`
/// entry that matches, `candidates` already sorted longest-first), or return
/// `line` unchanged when none matches. Only ever applied to a whole `Grep`/
/// `Glob` output *line*, whose path — when present at all — is always
/// exactly the line's own prefix; this can never reach into `content`.
fn strip_leading_root(line: &str, candidates: &[PathBuf]) -> String {
    for candidate in candidates {
        let prefix = candidate.to_string_lossy();
        if prefix.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix(format!("{prefix}/").as_str()) {
            return rest.to_string();
        }
    }
    line.to_string()
}

/// The macOS top-level directories symlinked into `/private`.
const MACOS_PRIVATE_ALIASED: &[&str] = &["tmp", "var", "etc"];

/// Both spellings of `path` that could appear verbatim in tool output:
/// `path` itself, and — if it starts with `/private/<tmp|var|etc>` or bare
/// `/<tmp|var|etc>` — the other spelling of the same location. Unlike
/// [`super::plan_file::normalize_macos_private_prefix`] (which normalizes
/// both sides of a *comparison* down to one form), this needs both literal
/// spellings, since it drives a text substitution rather than an equality
/// check.
fn macos_spellings(path: &Path) -> Vec<PathBuf> {
    let mut out = vec![path.to_path_buf()];
    let components: Vec<Component> = path.components().collect();

    let is_aliased_name = |c: Option<&Component>| matches!(c, Some(Component::Normal(n)) if n.to_str().is_some_and(|s| MACOS_PRIVATE_ALIASED.contains(&s)));

    if matches!(components.first(), Some(Component::RootDir)) {
        let second_is_private =
            matches!(components.get(1), Some(Component::Normal(n)) if *n == "private");
        if second_is_private && is_aliased_name(components.get(2)) {
            // `/private/<x>/...` -> `/<x>/...`
            let mut alt = PathBuf::from("/");
            for c in &components[2..] {
                alt.push(c.as_os_str());
            }
            out.push(alt);
        } else if is_aliased_name(components.get(1)) {
            // `/<x>/...` -> `/private/<x>/...`
            let mut alt = PathBuf::from("/private");
            for c in &components[1..] {
                alt.push(c.as_os_str());
            }
            out.push(alt);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Windowing / truncation detection
// ---------------------------------------------------------------------------

/// A `Read` is windowed when its stored input explicitly carries
/// `offset`/`limit` (a captured call always omits both entirely for a
/// whole-file read — see `plan_file::relativize`'s null-stripping), or when
/// an unwindowed call's output happens to hit the tool's 2000-line default
/// exactly (see the module docs on why that's a coincidental false positive
/// we accept rather than try to disambiguate).
fn is_windowed_read(input: &Value, normalized_output: &str) -> bool {
    let carries_window = input.get("offset").is_some_and(|v| !v.is_null())
        || input.get("limit").is_some_and(|v| !v.is_null());
    carries_window || normalized_output.lines().count() == READ_DEFAULT_LIMIT
}

/// `Grep` gives no positive signal when it hits its cap (unlike `Glob`'s
/// notice) — see the module docs.
fn is_truncated_grep(output: &str) -> bool {
    output != GREP_NO_MATCHES && output.lines().count() == GREP_MATCH_CAP
}

/// `Glob` embeds its truncation notice directly in the output when it hits
/// its (possibly caller-overridden) limit.
fn is_truncated_glob(output: &str) -> bool {
    output.contains(GLOB_TRUNCATION_NOTICE)
}

// ---------------------------------------------------------------------------
// Checksums
// ---------------------------------------------------------------------------

/// The only input keys cersei's `GrepTool` recognizes at all
/// (`grep_tool.rs`'s `input_schema`/`execute`'s local `Input` struct:
/// `pattern`, `path`, `glob`, `case_insensitive`, `hidden` — verified against
/// the pinned source, commit `e75278d2d22d67674c154512de6d43c4161139bf`).
/// `GrepTool` has exactly **one** output shape — `path:line:content` triples,
/// or the literal `"No matches found."` — and its `Input` struct carries no
/// `#[serde(deny_unknown_fields)]`, so `serde_json::from_value` silently
/// *ignores* any other key rather than honoring it: there is no
/// `output_mode`/context-lines (`-A`/`-B`/`-C`)/`head_limit`/multiline mode
/// in this tool at all (unlike some other "Grep" tool schemas this module's
/// author might otherwise assume). See [`grep_input_is_understood`] for how
/// this allowlist is still enforced defensively.
const GREP_KNOWN_INPUT_KEYS: &[&str] = &["pattern", "path", "glob", "case_insensitive", "hidden"];

/// Whether `input` only uses keys [`GREP_KNOWN_INPUT_KEYS`] — i.e. whether
/// `compute_checksums` can trust the familiar `path:line:content` shape.
/// Nothing reachable through cersei's current `GrepTool` can make this
/// `false` (see [`GREP_KNOWN_INPUT_KEYS`]'s docs), but a future cersei
/// version — or a hand-edited/corrupted plan — adding a recognized-looking
/// key this parser doesn't actually handle must not be silently misparsed
/// into a wrong-but-stable checksum, per this module's guiding principle.
fn grep_input_is_understood(input: &Value) -> bool {
    input.as_object().is_some_and(|obj| {
        obj.keys()
            .all(|k| GREP_KNOWN_INPUT_KEYS.contains(&k.as_str()))
    })
}

fn compute_checksums(tool: ReadOnlyTool, input: &Value, normalized_output: &str) -> CallChecksum {
    match tool {
        ReadOnlyTool::Read | ReadOnlyTool::Glob => {
            CallChecksum::Single(xxh64_hex(normalized_output.as_bytes()))
        }
        ReadOnlyTool::Grep if grep_input_is_understood(input) => {
            let (files, lines) = grep_checksum_parts(normalized_output);
            CallChecksum::Grep {
                files_xxh64: xxh64_hex(files.as_bytes()),
                lines_xxh64: xxh64_hex(lines.as_bytes()),
            }
        }
        ReadOnlyTool::Grep => {
            // Opaque fallback: an input this parser doesn't understand might
            // not actually produce `path:line:content` (today, nothing
            // reachable through `GrepTool` triggers this — see
            // `GREP_KNOWN_INPUT_KEYS` — but replay must not assume that holds
            // forever). Sorted lines, then one hash duplicated into both
            // halves: `classify_call`'s existing `files_xxh64` comparison
            // alone then already forces `DiscoveryStale` on *any* change and
            // `Fresh` only when byte-identical — never a false
            // `Fresh`/`ReadsStale` from a shape this module doesn't parse.
            let mut lines: Vec<&str> = normalized_output.lines().collect();
            lines.sort_unstable();
            let whole = xxh64_hex(lines.join("\n").as_bytes());
            CallChecksum::Grep {
                files_xxh64: whole.clone(),
                lines_xxh64: whole,
            }
        }
    }
}

/// Split Grep's normalized (root-prefix-already-stripped) output into the two
/// halves [`compute_checksums`] hashes separately: the sorted, deduped set of
/// matched paths (`files`) and every `path:content` pair with line numbers
/// stripped, sorted for stability (`lines`). [`GREP_NO_MATCHES`] is the empty
/// set for both.
fn grep_checksum_parts(output: &str) -> (String, String) {
    if output == GREP_NO_MATCHES {
        return (String::new(), String::new());
    }

    let mut files: Vec<String> = Vec::new();
    let mut line_pairs: Vec<String> = Vec::new();
    for line in output.lines() {
        // A line that doesn't match cersei's own `path:line:content` shape
        // can't happen for real Grep output; fall back to the whole line as
        // its own "path" so it still affects the checksum rather than
        // silently vanishing.
        let (path, content) = split_grep_line(line).unwrap_or((line, ""));
        files.push(path.to_string());
        line_pairs.push(format!("{path}:{content}"));
    }

    files.sort();
    files.dedup();
    // Sorted (not left in output order): `GrepTool`'s own output is already
    // stably sorted by `(file, line)` for a non-truncated result (cersei
    // sorts before truncating — see `tool_primitives::search::grep_blocking`),
    // but sorting explicitly here makes `lines_xxh64` correct without
    // depending on that as an unstated contract of a dependency this repo
    // doesn't own.
    line_pairs.sort();

    (files.join("\n"), line_pairs.join("\n"))
}

/// Split one Grep line (`path:line:content`) into `(path, content)` by
/// locating the *first* `:<digits>:` in the line. This is deliberately a
/// left-to-right scan rather than a naive `:`-split: the true separator is
/// the line's own prefix, so it is always found before any `:<digits>:`-
/// shaped text that happens to appear inside `content` later in the line.
///
/// **Known limitation, not fixed here (tracked as a follow-up):** the ticket
/// for this module explicitly prescribes "first `:<digits>:`", and that is
/// exactly what this does — but a matched *path* that itself contains a
/// `:<digits>:`-shaped substring *before* the real separator (e.g.
/// `a:12:b.rs:34:real content`) mis-splits: `path` comes out as `a`, not
/// `a:12:b.rs`. `:` isn't a valid path character on Windows at all, and is
/// rare in practice elsewhere, but this is a real, known gap — not a "can't
/// happen" case — left as-is per the orchestrator's explicit instruction not
/// to address it in this ticket.
fn split_grep_line(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b':' {
                return Some((&line[..i], &line[j + 1..]));
            }
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    // -- fixtures / helpers ---------------------------------------------------

    fn write(dir: &TempDir, relative: &str, content: &str) {
        let path = dir.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    /// Replay `tool`/`input` against `dir` and build the `PlanCall` a plan
    /// would have frozen for it — the baseline a later, post-mutation replay
    /// gets classified against. Panics if the baseline call itself is
    /// truncated or missing (a broken test fixture, not something under
    /// test).
    async fn baseline(tool: ReadOnlyTool, input: Value, root: &Path) -> PlanCall {
        replay_call(tool, &input, root)
            .await
            .to_plan_call()
            .expect("baseline replay must succeed (not truncated/missing)")
    }

    async fn check_freshness(stored: &PlanCall, root: &Path) -> Freshness {
        replay_check("check", std::slice::from_ref(stored), root)
            .await
            .freshness
    }

    /// Scopes a process cwd change for the duration of a test, restoring the
    /// previous cwd on drop (including on panic/early return). Safe under
    /// `cargo nextest` (this repo's canonical test runner — see
    /// `CLAUDE.md`), which process-isolates each test; NOT safe to run
    /// concurrently with another cwd-mutating test under the standard
    /// `cargo test` harness, which shares one process across threads.
    struct CwdGuard {
        previous: PathBuf,
    }

    impl CwdGuard {
        fn enter(dir: &Path) -> Self {
            let previous = std::env::current_dir().unwrap();
            std::env::set_current_dir(dir).unwrap();
            Self { previous }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.previous);
        }
    }

    /// A minimal [`tracing::Subscriber`] that counts `WARN` events emitted
    /// from this module, for the truncation tests' "exactly one warn log"
    /// assertion. No existing log-assertion helper/dev-dependency was found
    /// elsewhere in this repo (checked before adding one) — `tracing`'s own
    /// `Subscriber` trait is enough to count events without a new crate.
    #[derive(Clone, Default)]
    struct WarnCounter(Arc<AtomicUsize>);

    impl WarnCounter {
        fn count(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    impl tracing::Subscriber for WarnCounter {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() == tracing::Level::WARN
                && event.metadata().target().contains("checks::jev::replay")
            {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn enter(&self, _span: &tracing::span::Id) {}
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    // -- Read: freshness scenarios --------------------------------------------

    #[tokio::test]
    async fn unchanged_whole_file_read_is_fresh() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "fn main() {}\n");
        let input = json!({"file_path": "src/lib.rs"});
        let stored = baseline(ReadOnlyTool::Read, input, dir.path()).await;

        assert_eq!(check_freshness(&stored, dir.path()).await, Freshness::Fresh);
    }

    #[tokio::test]
    async fn edited_whole_file_read_is_reads_stale() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "fn main() {}\n");
        let input = json!({"file_path": "src/lib.rs"});
        let stored = baseline(ReadOnlyTool::Read, input, dir.path()).await;

        write(&dir, "src/lib.rs", "fn main() { println!(\"hi\"); }\n");

        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::ReadsStale
        );
    }

    #[tokio::test]
    async fn deleted_file_behind_a_read_is_discovery_stale() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "fn main() {}\n");
        let input = json!({"file_path": "src/lib.rs"});
        let stored = baseline(ReadOnlyTool::Read, input, dir.path()).await;

        std::fs::remove_file(dir.path().join("src/lib.rs")).unwrap();

        let replayed = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": "src/lib.rs"}),
            dir.path(),
        )
        .await;
        assert!(replayed.missing);
        assert_eq!(
            classify_call(&stored, &replayed),
            Some(Freshness::DiscoveryStale)
        );
        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::DiscoveryStale
        );
    }

    #[tokio::test]
    async fn edited_file_behind_a_windowed_read_is_discovery_stale() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "one\ntwo\nthree\nfour\n");
        let input = json!({"file_path": "src/lib.rs", "offset": 0, "limit": 2});
        let stored = baseline(ReadOnlyTool::Read, input, dir.path()).await;

        write(&dir, "src/lib.rs", "ONE\ntwo\nthree\nfour\n");

        let replayed = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": "src/lib.rs", "offset": 0, "limit": 2}),
            dir.path(),
        )
        .await;
        assert!(replayed.windowed);
        assert_eq!(
            classify_call(&stored, &replayed),
            Some(Freshness::DiscoveryStale)
        );
    }

    #[tokio::test]
    async fn a_read_omitting_offset_and_limit_is_not_windowed() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "hi\n");
        let replayed = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": "a.txt"}),
            dir.path(),
        )
        .await;
        assert!(!replayed.windowed);
    }

    #[tokio::test]
    async fn a_read_hitting_the_default_line_cap_is_windowed() {
        let dir = TempDir::new().unwrap();
        let body: String = (0..READ_DEFAULT_LIMIT + 5)
            .map(|i| format!("{i}\n"))
            .collect();
        write(&dir, "big.txt", &body);
        let replayed = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": "big.txt"}),
            dir.path(),
        )
        .await;
        assert!(
            !replayed.truncated,
            "Read has no truncated state, only windowed"
        );
        assert!(replayed.windowed);
    }

    // -- Glob: freshness scenarios --------------------------------------------

    #[tokio::test]
    async fn unchanged_glob_is_fresh() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "");
        let input = json!({"pattern": "**/*.rs", "path": "."});
        let stored = baseline(ReadOnlyTool::Glob, input, dir.path()).await;

        assert_eq!(check_freshness(&stored, dir.path()).await, Freshness::Fresh);
    }

    #[tokio::test]
    async fn a_new_file_matching_a_frozen_glob_is_discovery_stale() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "");
        let input = json!({"pattern": "**/*.rs", "path": "."});
        let stored = baseline(ReadOnlyTool::Glob, input, dir.path()).await;

        write(&dir, "src/new.rs", "");

        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::DiscoveryStale
        );
    }

    // -- Grep: freshness scenarios --------------------------------------------

    #[tokio::test]
    async fn line_inserted_above_a_grep_match_is_fresh() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "one\nTARGET two\nthree\n");
        let input = json!({"pattern": "TARGET", "path": "."});
        let stored = baseline(ReadOnlyTool::Grep, input, dir.path()).await;

        // Same matched line content, shifted down by one line number.
        write(&dir, "a.txt", "zero\none\nTARGET two\nthree\n");

        assert_eq!(check_freshness(&stored, dir.path()).await, Freshness::Fresh);
    }

    #[tokio::test]
    async fn a_matched_lines_text_edited_is_reads_stale() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "one\nTARGET two\nthree\n");
        let input = json!({"pattern": "TARGET", "path": "."});
        let stored = baseline(ReadOnlyTool::Grep, input, dir.path()).await;

        write(&dir, "a.txt", "one\nTARGET twoo\nthree\n");

        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::ReadsStale
        );
    }

    #[tokio::test]
    async fn a_new_match_in_an_already_matched_file_is_reads_stale() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "TARGET one\n");
        let input = json!({"pattern": "TARGET", "path": "."});
        let stored = baseline(ReadOnlyTool::Grep, input, dir.path()).await;

        write(&dir, "a.txt", "TARGET one\nTARGET two\n");

        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::ReadsStale
        );
    }

    #[tokio::test]
    async fn a_match_in_a_previously_unmatched_file_is_discovery_stale() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "TARGET one\n");
        let input = json!({"pattern": "TARGET", "path": "."});
        let stored = baseline(ReadOnlyTool::Grep, input, dir.path()).await;

        write(&dir, "b.txt", "TARGET two\n");

        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::DiscoveryStale
        );
    }

    #[tokio::test]
    async fn first_match_after_no_matches_found_is_discovery_stale() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "nothing interesting\n");
        let input = json!({"pattern": "TARGET", "path": "."});
        let stored = baseline(ReadOnlyTool::Grep, input, dir.path()).await;

        write(&dir, "a.txt", "TARGET now present\n");

        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::DiscoveryStale
        );
    }

    #[tokio::test]
    async fn only_line_numbers_shifting_leaves_grep_checksums_unchanged() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "one\nTARGET two\nthree\n");
        let before = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "TARGET", "path": "."}),
            dir.path(),
        )
        .await;

        write(&dir, "a.txt", "zero\none\nTARGET two\nthree\n");
        let after = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "TARGET", "path": "."}),
            dir.path(),
        )
        .await;

        assert_eq!(before.checksums, after.checksums);
    }

    // -- Grep line parsing: the colon traps -----------------------------------

    #[test]
    fn splits_a_well_formed_grep_line() {
        assert_eq!(
            split_grep_line("src/lib.rs:42:fn main() {}"),
            Some(("src/lib.rs", "fn main() {}"))
        );
    }

    #[test]
    fn splits_correctly_when_content_contains_colons() {
        assert_eq!(
            split_grep_line("src/lib.rs:10:let x: Result<(), Error> = f();"),
            Some(("src/lib.rs", "let x: Result<(), Error> = f();"))
        );
    }

    #[test]
    fn splits_correctly_when_content_contains_a_colon_digits_colon_lookalike() {
        // The `:99:` inside `content` must not be mistaken for the real
        // separator: the true `path:line:` prefix is always found first by a
        // left-to-right scan.
        assert_eq!(
            split_grep_line("src/lib.rs:5:see also line :99: in the other file"),
            Some(("src/lib.rs", "see also line :99: in the other file"))
        );
    }

    #[test]
    fn splits_correctly_when_the_path_itself_contains_a_colon() {
        assert_eq!(
            split_grep_line("weird:name.rs:7:content here"),
            Some(("weird:name.rs", "content here"))
        );
    }

    #[tokio::test]
    async fn grep_content_containing_colons_parses_and_checksums_correctly() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.rs", "let x: Result<(), Error> = f(); // TARGET\n");
        let input = json!({"pattern": "TARGET", "path": "."});
        let stored = baseline(ReadOnlyTool::Grep, input, dir.path()).await;

        // Unchanged: fresh.
        assert_eq!(check_freshness(&stored, dir.path()).await, Freshness::Fresh);

        // Edit inside the colon-bearing content: reads-stale, not silently
        // dropped or mis-attributed to a different "file".
        write(
            &dir,
            "a.rs",
            "let x: Result<(), OtherError> = f(); // TARGET\n",
        );
        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::ReadsStale
        );
    }

    // -- Truncation ------------------------------------------------------------

    #[tokio::test]
    async fn grep_over_the_match_cap_warns_once_and_is_discovery_stale_when_previously_checksummed()
    {
        // A previously fresh (non-truncated), checksummed call that NOW hits
        // the cap is discovery that grew past the boundary since the plan
        // was frozen — must be `DiscoveryStale`, not silently excluded (the
        // original bug: excluding *any* truncated call unconditionally let
        // this aggregate to `Fresh`).
        let dir = TempDir::new().unwrap();
        for i in 0..(GREP_MATCH_CAP + 50) {
            write(&dir, &format!("f{i:04}.txt"), "NEEDLE\n");
        }
        let stored = PlanCall::Grep {
            input: json!({"pattern": "NEEDLE", "path": "."}),
            files_xxh64: "0".repeat(16),
            lines_xxh64: "0".repeat(16),
        };

        let counter = WarnCounter::default();
        let freshness = {
            // `set_default` installs a thread-local default for as long as
            // the guard lives; safe across the `.await` points below because
            // `#[tokio::test]` defaults to a current-thread runtime, so this
            // whole `async fn` never hops OS threads.
            let _guard = tracing::subscriber::set_default(counter.clone());
            check_freshness(&stored, dir.path()).await
        };

        assert_eq!(freshness, Freshness::DiscoveryStale);
        assert_eq!(
            counter.count(),
            1,
            "still warns exactly once on the detection"
        );

        let replayed = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "NEEDLE", "path": "."}),
            dir.path(),
        )
        .await;
        assert!(replayed.truncated);
        assert!(replayed.checksums.is_none());
    }

    #[tokio::test]
    async fn grep_over_the_match_cap_is_excluded_when_the_plan_already_knew_it_was_truncated() {
        // Both sides agree the call is truncated: genuinely nothing
        // comparable, so it's excluded from freshness (but still warns).
        let dir = TempDir::new().unwrap();
        for i in 0..(GREP_MATCH_CAP + 50) {
            write(&dir, &format!("f{i:04}.txt"), "NEEDLE\n");
        }
        let stored = PlanCall::Truncated {
            tool: ReadOnlyTool::Grep,
            input: json!({"pattern": "NEEDLE", "path": "."}),
        };

        let counter = WarnCounter::default();
        let freshness = {
            let _guard = tracing::subscriber::set_default(counter.clone());
            check_freshness(&stored, dir.path()).await
        };

        assert_eq!(
            freshness,
            Freshness::Fresh,
            "excluded, so nothing else to escalate it"
        );
        assert_eq!(counter.count(), 1);
    }

    #[tokio::test]
    async fn glob_over_the_default_limit_warns_once_and_is_discovery_stale_when_previously_checksummed()
     {
        let dir = TempDir::new().unwrap();
        for i in 0..250 {
            write(&dir, &format!("f{i:04}.rs"), "");
        }
        let stored = PlanCall::Glob {
            input: json!({"pattern": "*.rs", "path": "."}),
            xxh64: "0".repeat(16),
        };

        let counter = WarnCounter::default();
        let freshness = {
            let _guard = tracing::subscriber::set_default(counter.clone());
            check_freshness(&stored, dir.path()).await
        };

        assert_eq!(freshness, Freshness::DiscoveryStale);
        assert_eq!(
            counter.count(),
            1,
            "still warns exactly once on the detection"
        );
    }

    #[tokio::test]
    async fn glob_over_the_default_limit_is_excluded_when_the_plan_already_knew_it_was_truncated() {
        let dir = TempDir::new().unwrap();
        for i in 0..250 {
            write(&dir, &format!("f{i:04}.rs"), "");
        }
        let stored = PlanCall::Truncated {
            tool: ReadOnlyTool::Glob,
            input: json!({"pattern": "*.rs", "path": "."}),
        };

        let counter = WarnCounter::default();
        let freshness = {
            let _guard = tracing::subscriber::set_default(counter.clone());
            check_freshness(&stored, dir.path()).await
        };

        assert_eq!(
            freshness,
            Freshness::Fresh,
            "excluded, so nothing else to escalate it"
        );
        assert_eq!(counter.count(), 1);
    }

    #[tokio::test]
    async fn glob_at_exactly_its_limit_is_not_truncated() {
        let dir = TempDir::new().unwrap();
        for i in 0..10 {
            write(&dir, &format!("f{i:02}.rs"), "");
        }
        let replayed = replay_call(
            ReadOnlyTool::Glob,
            &json!({"pattern": "*.rs", "path": ".", "limit": 10}),
            dir.path(),
        )
        .await;
        assert!(!replayed.truncated);
    }

    #[tokio::test]
    async fn grep_at_exactly_the_cap_is_conservatively_truncated() {
        // A known, accepted false positive (see the module docs): the tool
        // gives no signal distinguishing "exactly 250 matches" from "more
        // than 250, truncated at 250".
        let dir = TempDir::new().unwrap();
        for i in 0..GREP_MATCH_CAP {
            write(&dir, &format!("f{i:04}.txt"), "NEEDLE\n");
        }
        let replayed = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "NEEDLE", "path": "."}),
            dir.path(),
        )
        .await;
        assert!(replayed.truncated);
    }

    // -- classify_call: truncation-status and tool-mismatch safety -------------
    //
    // Direct unit tests against `classify_call`, hand-building the
    // `ReplayedCall`/`PlanCall` pairings under test rather than driving them
    // through a real replay, since the whole point is to exercise
    // combinations `replay_check` itself would never produce.

    fn checksummed_grep_call(files_xxh64: &str, lines_xxh64: &str) -> PlanCall {
        PlanCall::Grep {
            input: json!({"pattern": "TARGET", "path": "."}),
            files_xxh64: files_xxh64.to_string(),
            lines_xxh64: lines_xxh64.to_string(),
        }
    }

    fn checksummed_replayed_grep(files_xxh64: &str, lines_xxh64: &str) -> ReplayedCall {
        ReplayedCall {
            tool: ReadOnlyTool::Grep,
            input: json!({"pattern": "TARGET", "path": "."}),
            output: Some("a.rs:1:TARGET".to_string()),
            checksums: Some(CallChecksum::Grep {
                files_xxh64: files_xxh64.to_string(),
                lines_xxh64: lines_xxh64.to_string(),
            }),
            windowed: false,
            truncated: false,
            missing: false,
        }
    }

    fn truncated_replayed_grep() -> ReplayedCall {
        ReplayedCall {
            tool: ReadOnlyTool::Grep,
            input: json!({"pattern": "TARGET", "path": "."}),
            output: Some("...".to_string()),
            checksums: None,
            windowed: false,
            truncated: true,
            missing: false,
        }
    }

    fn truncated_stored_grep() -> PlanCall {
        PlanCall::Truncated {
            tool: ReadOnlyTool::Grep,
            input: json!({"pattern": "TARGET", "path": "."}),
        }
    }

    #[test]
    fn a_previously_checksummed_call_that_now_replays_truncated_is_discovery_stale() {
        // Discovery grew past the cap since the plan was frozen — must not
        // be silently excluded from freshness.
        let stored = checksummed_grep_call("aaaa000000000000", "bbbb000000000000");
        let replayed = truncated_replayed_grep();
        assert_eq!(
            classify_call(&stored, &replayed),
            Some(Freshness::DiscoveryStale)
        );
    }

    #[test]
    fn a_previously_truncated_call_that_now_replays_normally_is_discovery_stale() {
        // We have no stored checksum to compare the now-clean replay
        // against — can't be verified fresh, must escalate.
        let stored = truncated_stored_grep();
        let replayed = checksummed_replayed_grep("aaaa000000000000", "bbbb000000000000");
        assert_eq!(
            classify_call(&stored, &replayed),
            Some(Freshness::DiscoveryStale)
        );
    }

    #[test]
    fn truncated_on_both_sides_is_excluded_from_freshness() {
        let stored = truncated_stored_grep();
        let replayed = truncated_replayed_grep();
        assert_eq!(classify_call(&stored, &replayed), None);
    }

    #[test]
    fn a_tool_shape_mismatch_between_stored_and_replayed_is_never_fresh() {
        // A `Read`/`Glob` stored call paired against a `Glob`/`Read`-shaped
        // replay could otherwise coincidentally match via
        // `CallChecksum::Single` — must never read as `Fresh`.
        let stored = PlanCall::Read {
            input: json!({"file_path": "a.rs"}),
            xxh64: "deadbeefdeadbeef".to_string(),
        };
        let replayed = ReplayedCall {
            tool: ReadOnlyTool::Glob,
            input: json!({"pattern": "*.rs", "path": "."}),
            output: Some("a.rs".to_string()),
            // Same checksum value as `stored`'s — a coincidental collision
            // this test specifically constructs to prove it still isn't
            // trusted.
            checksums: Some(CallChecksum::Single("deadbeefdeadbeef".to_string())),
            windowed: false,
            truncated: false,
            missing: false,
        };
        assert_eq!(
            classify_call(&stored, &replayed),
            Some(Freshness::DiscoveryStale)
        );
    }

    #[test]
    fn missing_wins_over_a_stored_truncated_call() {
        let stored = truncated_stored_grep();
        let replayed = ReplayedCall {
            tool: ReadOnlyTool::Grep,
            input: json!({"pattern": "TARGET", "path": "."}),
            output: None,
            checksums: None,
            windowed: false,
            truncated: false,
            missing: true,
        };
        assert_eq!(
            classify_call(&stored, &replayed),
            Some(Freshness::DiscoveryStale)
        );
    }

    // -- Missing / jail rejection ----------------------------------------------

    #[tokio::test]
    async fn a_stored_relative_path_escaping_the_root_via_dot_dot_is_rejected_by_the_jail() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.rs"), "top secret").unwrap();

        let escape = format!(
            "../{}/secret.rs",
            outside.path().file_name().unwrap().to_str().unwrap()
        );
        let replayed = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": escape}),
            root.path(),
        )
        .await;

        assert!(replayed.missing, "the jail must deny the escaping path");
        assert!(replayed.output.is_none());
    }

    #[tokio::test]
    async fn a_stored_absolute_path_outside_the_root_is_rejected_by_the_jail() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.rs"), "top secret").unwrap();

        let replayed = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": outside.path().join("secret.rs").to_string_lossy()}),
            root.path(),
        )
        .await;

        assert!(replayed.missing);
    }

    #[tokio::test]
    async fn a_stored_grep_path_escaping_the_root_is_rejected_by_the_jail() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.rs"), "TARGET\n").unwrap();

        let escape = format!(
            "../{}",
            outside.path().file_name().unwrap().to_str().unwrap()
        );
        let replayed = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "TARGET", "path": escape}),
            root.path(),
        )
        .await;

        assert!(
            replayed.missing,
            "the jail must deny a Grep path escaping the root"
        );
        assert_eq!(
            classify_call(
                &PlanCall::Grep {
                    input: json!({"pattern": "TARGET", "path": "."}),
                    files_xxh64: "0".repeat(16),
                    lines_xxh64: "0".repeat(16),
                },
                &replayed
            ),
            Some(Freshness::DiscoveryStale)
        );
    }

    #[tokio::test]
    async fn a_stored_glob_path_escaping_the_root_is_rejected_by_the_jail() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        write(&outside, "secret.rs", "");

        let escape = format!(
            "../{}",
            outside.path().file_name().unwrap().to_str().unwrap()
        );
        let replayed = replay_call(
            ReadOnlyTool::Glob,
            &json!({"pattern": "*.rs", "path": escape}),
            root.path(),
        )
        .await;

        assert!(
            replayed.missing,
            "the jail must deny a Glob path escaping the root"
        );
    }

    #[tokio::test]
    async fn a_stored_absolute_glob_pattern_escaping_the_root_is_rejected_by_the_jail() {
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        write(&outside, "secret.rs", "");

        let absolute_pattern = outside.path().join("*.rs").to_string_lossy().into_owned();
        let replayed = replay_call(
            ReadOnlyTool::Glob,
            &json!({"pattern": absolute_pattern, "path": "."}),
            root.path(),
        )
        .await;

        assert!(
            replayed.missing,
            "the jail must deny an absolute Glob pattern escaping the root"
        );
    }

    // -- Machine independence ---------------------------------------------------

    #[tokio::test]
    async fn identical_trees_at_different_roots_produce_identical_checksums() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        for dir in [&dir_a, &dir_b] {
            write(dir, "src/lib.rs", "fn main() {}\n");
            write(dir, "src/other.rs", "TARGET here\n");
        }

        let read_a = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": "src/lib.rs"}),
            dir_a.path(),
        )
        .await;
        let read_b = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": "src/lib.rs"}),
            dir_b.path(),
        )
        .await;
        assert_eq!(read_a.checksums, read_b.checksums);

        let glob_a = replay_call(
            ReadOnlyTool::Glob,
            &json!({"pattern": "**/*.rs", "path": "."}),
            dir_a.path(),
        )
        .await;
        let glob_b = replay_call(
            ReadOnlyTool::Glob,
            &json!({"pattern": "**/*.rs", "path": "."}),
            dir_b.path(),
        )
        .await;
        assert_eq!(glob_a.checksums, glob_b.checksums);
        assert_eq!(glob_a.output, glob_b.output);

        let grep_a = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "TARGET", "path": "."}),
            dir_a.path(),
        )
        .await;
        let grep_b = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "TARGET", "path": "."}),
            dir_b.path(),
        )
        .await;
        assert_eq!(grep_a.checksums, grep_b.checksums);
        assert_eq!(grep_a.output, grep_b.output);
    }

    #[tokio::test]
    async fn checksums_are_stable_across_repeated_replays() {
        let dir = TempDir::new().unwrap();
        write(&dir, "src/lib.rs", "TARGET one\n");
        write(&dir, "src/other.rs", "TARGET two\nTARGET three\n");
        let input = json!({"pattern": "TARGET", "path": "."});

        let first = replay_call(ReadOnlyTool::Grep, &input, dir.path()).await;
        let second = replay_call(ReadOnlyTool::Grep, &input, dir.path()).await;
        let third = replay_call(ReadOnlyTool::Grep, &input, dir.path()).await;

        assert_eq!(first.checksums, second.checksums);
        assert_eq!(second.checksums, third.checksums);
    }

    #[tokio::test]
    async fn glob_checksum_is_independent_of_file_creation_order_and_mtime() {
        // Verified against the pinned source
        // (`tool_primitives::search::glob_blocking`, `search.rs:332`):
        // `GlobTool` sorts its results lexicographically by `PathBuf`
        // *before* truncating — never by modification time or walk/creation
        // order — so a non-truncated `Glob` checksum must already be stable
        // across both. This test proves it rather than only asserting it.
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();

        write(&dir_a, "a.rs", "");
        write(&dir_a, "b.rs", "");
        write(&dir_a, "c.rs", "");

        // Same three files, created in the opposite order.
        write(&dir_b, "c.rs", "");
        write(&dir_b, "b.rs", "");
        write(&dir_b, "a.rs", "");
        // Rewritten again (bumping its mtime) without changing its content.
        std::thread::sleep(std::time::Duration::from_millis(5));
        write(&dir_b, "a.rs", "");

        let input = json!({"pattern": "*.rs", "path": "."});
        let glob_a = replay_call(ReadOnlyTool::Glob, &input, dir_a.path()).await;
        let glob_b = replay_call(ReadOnlyTool::Glob, &input, dir_b.path()).await;

        assert_eq!(glob_a.checksums, glob_b.checksums);
    }

    // -- Invocation independence --------------------------------------------

    #[tokio::test]
    async fn replay_never_depends_on_process_cwd() {
        let dir = TempDir::new().unwrap();
        write(&dir, "sub/marker.txt", "x");
        write(&dir, "top.rs", "TARGET\n");
        let input = json!({"pattern": "TARGET", "path": "."});

        let from_root = {
            let _cwd = CwdGuard::enter(dir.path());
            replay_call(ReadOnlyTool::Grep, &input, dir.path()).await
        };
        let from_sub = {
            let _cwd = CwdGuard::enter(&dir.path().join("sub"));
            replay_call(ReadOnlyTool::Grep, &input, dir.path()).await
        };

        assert!(!from_root.missing && !from_sub.missing);
        assert_eq!(from_root.checksums, from_sub.checksums);
        assert_eq!(from_root.output, from_sub.output);
    }

    // -- build_tool lockstep --------------------------------------------------

    #[test]
    fn build_tool_matches_the_wire_name_for_every_read_only_tool() {
        assert_eq!(build_tool(ReadOnlyTool::Read).name(), "Read");
        assert_eq!(build_tool(ReadOnlyTool::Grep).name(), "Grep");
        assert_eq!(build_tool(ReadOnlyTool::Glob).name(), "Glob");
    }

    // -- output normalization --------------------------------------------------

    #[test]
    fn normalize_output_strips_the_raw_root_prefix() {
        let root = Path::new("/sandbox/root");
        let content = "/sandbox/root/src/lib.rs:1:fn main() {}";
        assert_eq!(
            normalize_output(ReadOnlyTool::Grep, content, root),
            "src/lib.rs:1:fn main() {}"
        );
    }

    #[test]
    fn normalize_output_strips_either_macos_private_var_spelling() {
        let content_private = "/private/var/folders/xy/z/src/lib.rs";
        assert_eq!(
            normalize_output(
                ReadOnlyTool::Glob,
                content_private,
                Path::new("/var/folders/xy/z")
            ),
            "src/lib.rs"
        );

        let content_raw = "/var/folders/xy/z/src/lib.rs";
        assert_eq!(
            normalize_output(
                ReadOnlyTool::Glob,
                content_raw,
                Path::new("/private/var/folders/xy/z")
            ),
            "src/lib.rs"
        );
    }

    #[test]
    fn normalize_output_never_touches_read_content() {
        // `Read` output has no path at all — a root-shaped substring inside
        // file content must survive verbatim, not be treated as a path to
        // strip.
        let root = Path::new("/sandbox/root");
        let content = "   1\tsee /sandbox/root/foo for details\n";
        assert_eq!(normalize_output(ReadOnlyTool::Read, content, root), content);
    }

    #[test]
    fn normalize_output_only_strips_grep_leading_path_not_content() {
        // The root-shaped text inside `content` (after the second `:`) must
        // NOT be stripped — only the line's own leading path.
        let root = Path::new("/sandbox/root");
        let content = "/sandbox/root/a.rs:3:see /sandbox/root/foo for details";
        assert_eq!(
            normalize_output(ReadOnlyTool::Grep, content, root),
            "a.rs:3:see /sandbox/root/foo for details"
        );
    }

    // -- content preservation (end-to-end, not just normalize_output) ----------

    #[tokio::test]
    async fn read_output_preserves_content_that_looks_like_the_root_path() {
        let dir = TempDir::new().unwrap();
        let root_str = dir.path().display().to_string();
        write(&dir, "a.txt", &format!("see {root_str}/foo for details\n"));

        let replayed = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": "a.txt"}),
            dir.path(),
        )
        .await;
        let output = replayed.output.unwrap();
        assert!(
            output.contains(&root_str),
            "Read output must not have root-shaped content stripped: {output}"
        );
    }

    #[tokio::test]
    async fn editing_a_read_line_that_contains_the_root_path_text_is_reads_stale() {
        // Regression for the global-replace bug: collapsing `<root>/foo` and
        // `<root>/bar` both down to relative text made this edit invisible
        // to the checksum (a false `Fresh`).
        let dir = TempDir::new().unwrap();
        let root_str = dir.path().display().to_string();
        write(&dir, "a.txt", &format!("see {root_str}/foo for details\n"));
        let input = json!({"file_path": "a.txt"});
        let stored = baseline(ReadOnlyTool::Read, input, dir.path()).await;

        write(&dir, "a.txt", &format!("see {root_str}/bar for details\n"));

        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::ReadsStale
        );
    }

    #[tokio::test]
    async fn grep_content_that_contains_the_root_path_text_is_preserved() {
        let dir = TempDir::new().unwrap();
        let root_str = dir.path().display().to_string();
        write(
            &dir,
            "a.txt",
            &format!("TARGET see {root_str}/foo for details\n"),
        );

        let replayed = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "TARGET", "path": "."}),
            dir.path(),
        )
        .await;
        let output = replayed.output.unwrap();
        assert!(
            output.contains(&root_str),
            "Grep content must not have root-shaped text stripped: {output}"
        );
    }

    #[tokio::test]
    async fn editing_grep_content_that_contains_the_root_path_text_is_reads_stale() {
        let dir = TempDir::new().unwrap();
        let root_str = dir.path().display().to_string();
        write(
            &dir,
            "a.txt",
            &format!("TARGET see {root_str}/foo for details\n"),
        );
        let input = json!({"pattern": "TARGET", "path": "."});
        let stored = baseline(ReadOnlyTool::Grep, input, dir.path()).await;

        write(
            &dir,
            "a.txt",
            &format!("TARGET see {root_str}/bar for details\n"),
        );

        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::ReadsStale
        );
    }

    // -- Grep opaque fallback for an unrecognized input shape -------------------

    #[tokio::test]
    async fn grep_with_an_unrecognized_input_key_falls_back_to_a_whole_output_checksum() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.txt", "TARGET one\nTARGET two\n");
        // `head_limit` isn't a key `GrepTool` recognizes (verified against
        // the pinned source — see `GREP_KNOWN_INPUT_KEYS`); cersei's tool
        // silently ignores it and still returns the normal
        // `path:line:content` shape, but replay must not assume that.
        let input = json!({"pattern": "TARGET", "path": ".", "head_limit": 1});
        let stored = baseline(ReadOnlyTool::Grep, input, dir.path()).await;

        let PlanCall::Grep {
            files_xxh64,
            lines_xxh64,
            ..
        } = &stored
        else {
            panic!("expected a Grep PlanCall");
        };
        assert_eq!(
            files_xxh64, lines_xxh64,
            "the opaque fallback hashes both halves identically"
        );

        // A content-only edit (the matched file set is unchanged) must still
        // register as a `files_xxh64` difference in the opaque fallback —
        // never silently folded into a false `Fresh`/`ReadsStale`.
        write(&dir, "a.txt", "TARGET one\nTARGET twoo\n");
        assert_eq!(
            check_freshness(&stored, dir.path()).await,
            Freshness::DiscoveryStale
        );
    }

    #[test]
    fn grep_input_is_understood_accepts_every_recognized_key() {
        assert!(grep_input_is_understood(&json!({
            "pattern": "x",
            "path": ".",
            "glob": "*.rs",
            "case_insensitive": true,
            "hidden": false,
        })));
        assert!(grep_input_is_understood(&json!({"pattern": "x"})));
    }

    #[test]
    fn grep_input_is_understood_rejects_any_unrecognized_key() {
        assert!(!grep_input_is_understood(
            &json!({"pattern": "x", "head_limit": 1})
        ));
        assert!(!grep_input_is_understood(
            &json!({"pattern": "x", "output_mode": "files_with_matches"})
        ));
        assert!(!grep_input_is_understood(&json!("not an object")));
    }

    // -- CallChecksum / to_plan_call round trip --------------------------------

    #[tokio::test]
    async fn to_plan_call_round_trips_each_tool_shape() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.rs", "TARGET\n");

        let read = replay_call(
            ReadOnlyTool::Read,
            &json!({"file_path": "a.rs"}),
            dir.path(),
        )
        .await;
        assert!(matches!(read.to_plan_call(), Some(PlanCall::Read { .. })));

        let glob = replay_call(
            ReadOnlyTool::Glob,
            &json!({"pattern": "*.rs", "path": "."}),
            dir.path(),
        )
        .await;
        assert!(matches!(glob.to_plan_call(), Some(PlanCall::Glob { .. })));

        let grep = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "TARGET", "path": "."}),
            dir.path(),
        )
        .await;
        assert!(matches!(grep.to_plan_call(), Some(PlanCall::Grep { .. })));
    }

    #[tokio::test]
    async fn a_truncated_calls_to_plan_call_is_none() {
        let dir = TempDir::new().unwrap();
        for i in 0..(GREP_MATCH_CAP + 10) {
            write(&dir, &format!("f{i:04}.txt"), "NEEDLE\n");
        }
        let replayed = replay_call(
            ReadOnlyTool::Grep,
            &json!({"pattern": "NEEDLE", "path": "."}),
            dir.path(),
        )
        .await;
        assert!(replayed.to_plan_call().is_none());
    }

    // -- Zero calls -------------------------------------------------------------

    #[tokio::test]
    async fn a_check_with_no_calls_is_fresh() {
        let dir = TempDir::new().unwrap();
        let result = replay_check("empty check", &[], dir.path()).await;
        assert_eq!(result.freshness, Freshness::Fresh);
        assert!(result.calls.is_empty());
    }
}
