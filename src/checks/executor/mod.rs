//! The agent-executor seam (M2, narrowed in MULTI-1367). [`CheckExecutor`]
//! abstracts "run one check's agent → verdict/outcome". cersei-agent absorbs the
//! *provider-abstraction* rationale the seam originally carried, but not its
//! *test-seam* rationale: `cersei_agent::Agent` is a concrete struct, so the
//! execution-phase tests still need a fake. The trait keeps one method with two
//! impls — the real in-process [`cersei::CerseiExecutor`] and the test
//! [`FakeExecutor`]. It is a boxed trait object for dynamic dispatch, mirroring
//! the repo's `BoxedIngress` / `BoxedMonitor` / `BoxedPlatform` convention.

mod activity;
pub mod cersei;
#[cfg(test)]
mod fake;
// `pub(crate)` (not private): MULTI-1822's `checks::jev::replay` reuses
// `Jailed` verbatim to replay a plan's frozen calls through the same
// boundary check a check's live agent is confined by, rather than
// duplicating that logic — see `jail`'s module docs and `replay`'s.
pub(crate) mod jail;
pub mod judge;
mod tool_capture;
mod trace;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use miette::Result;

pub use judge::{CheckReport, JUDGE_TOOL};
pub use tool_capture::ToolCall;
// `ReadOnlyTool` (MULTI-1820's plan schema reuses it as the call-site `tool`
// enum) has no non-test consumer in the default build — only
// `crate::checks::jev::plan_file`, which exists only under `--features jev` —
// so re-exporting it unconditionally would be an unused `pub use` outside
// that feature. Test code within this crate reaches it via
// `tool_capture::ReadOnlyTool` directly regardless of this gate.
#[cfg(feature = "jev")]
pub use tool_capture::ReadOnlyTool;

use crate::checks::model::{Check, CheckId, DecidedBy, RootSource};
use crate::checks::sandbox::SandboxLease;

#[cfg(test)]
pub use fake::FakeExecutor;

/// Everything an executor needs to run one check's agent.
pub struct AgentRunRequest {
    /// The check this request runs, for routing/labelling and assembling the
    /// agent's instructions.
    pub check_id: CheckId,
    /// The check to validate (title + prompt). Each executor assembles its own
    /// instructions from this so it can describe its own reporting channel.
    pub check: Check,
    /// The requirement's repository root — the real, unsandboxed source
    /// directory (`job.root`; see
    /// [`Requirement::root`](crate::checks::model::Requirement::root)).
    /// Available without acquiring [`Self::sandbox`], so an executor that
    /// settles a check without running an agent (the Jev decision engine's
    /// in-host replay, MULTI-1825) can read evidence straight from here at
    /// zero sandboxing cost.
    pub source_dir: PathBuf,
    /// A lazy CoW sandbox lease over [`Self::source_dir`] (MULTI-1818). Call
    /// [`SandboxLease::acquire`] to create the clone — only agent-running
    /// executors need to; the clone is torn down when this request (and the
    /// lease within it) is dropped.
    pub sandbox: SandboxLease,
    /// The declaring `CHECKS.md`'s path relative to the requirement's
    /// repository root (MULTI-1834), e.g. `services/keystore/CHECKS.md`.
    /// Stated in the assembled instructions so the agent retains the scoping
    /// a smaller, per-scan-directory sandbox used to provide implicitly, now
    /// that the sandbox spans the requirement's whole repository root.
    pub declared_in: PathBuf,
    /// Which attempt this is, 1-based. Retries must not replay the failed
    /// attempt verbatim: executors use this to tell the agent a previous
    /// attempt went unreported and — on thinking-free runs — to raise the
    /// sampling temperature (the 2026-07-01 timeout postmortem showed
    /// temperature-0 retries reproducing the same fatal trajectory three
    /// times in a row).
    pub attempt: u32,
    /// Where a running agent reports its turn/activity progress, for the
    /// presenter (MULTI-1828). `None` in tests (and any executor) that don't
    /// care to surface it — progress is display-only, never required for
    /// correctness.
    pub progress: Option<ProgressSink>,
    /// This check's identity within its frozen plan (MULTI-1825) — see
    /// [`PlanIdentity`]. Populated unconditionally (by `execution::run_one`
    /// from `CheckJob`, and by `plan::planner::AgentPlanner` from
    /// `PlanRequest`) so every `AgentRunRequest { .. }` literal in this
    /// crate compiles in both feature sets; read only by the `jev` build's
    /// `JevExecutor`.
    pub plan: PlanIdentity,
}

/// A check's identity within its frozen `.check-plan.toml` (MULTI-1825): which
/// directory's plan covers it, its position within that plan
/// (`req_ordinal`/`check_ordinal` — the exact key
/// [`crate::checks::jev::plan_file::PlanFile::lookup`] looks an entry up by),
/// the owning requirement's title (sent to Jev as `state.requirement.title`),
/// and whether the requirement's root is even manifest-derived (a
/// [`RootSource::ScanDirectory`] root has no stable plan location, so
/// `JevExecutor` never reads or writes a plan for it — MULTI-1834's
/// scan-directory fallback).
#[derive(Debug, Clone)]
pub struct PlanIdentity {
    /// The directory `.check-plan.toml` lives beside — the declaring
    /// `CHECKS.md`'s own parent directory. See
    /// [`crate::checks::model::plan_dir_and_source`].
    pub dir: PathBuf,
    /// The declaring file's name (e.g. `"CHECKS.md"`) — see
    /// [`crate::checks::jev::plan_file::PlanRequirement::source`].
    pub source: String,
    /// This requirement's 0-based position within `source`.
    pub req_ordinal: u32,
    /// This check's 0-based position within its requirement.
    pub check_ordinal: u32,
    /// The owning requirement's title.
    pub requirement_title: String,
    /// How the requirement's repository root was determined (MULTI-1834).
    pub root_source: RootSource,
}

/// One progress update from a running check's agent (MULTI-1828), emitted
/// from [`cersei::CerseiExecutor`]'s `on_event` hook. Display-only: it never
/// reaches verdict, retry, or reporting logic — only the presenter.
#[derive(Debug, Clone)]
pub struct AgentProgress {
    /// The turn the agent is currently on (per `AgentEvent::TurnStart`).
    pub turn: u32,
    /// The executor's configured turn ceiling (e.g. `CerseiExecutor`'s
    /// `MAX_TURNS`), threaded through so the presenter can show `turn
    /// N/max` without hardcoding the limit itself.
    pub max_turns: u32,
    /// A short, root-relative rendering of the triggering allowlisted tool
    /// call (see [`activity::render_activity`]), when there is one to show.
    /// `None` for a bare turn start — a turn beginning has no activity yet.
    pub activity: Option<String>,
}

/// A cheap, cloneable, fire-and-forget sink for [`AgentProgress`] updates.
/// Backed by a small bounded channel: [`ProgressSink::send`] uses
/// `try_send`, so a slow or gone receiver can never block or fail the agent
/// run — a full or closed channel just drops the update, which is fine
/// because progress is display-only and the next update supersedes it
/// anyway.
#[derive(Clone)]
pub struct ProgressSink(tokio::sync::mpsc::Sender<AgentProgress>);

/// Small: these are ephemeral display updates, not a durable log — a burst
/// the presenter can't keep up with is fine to thin out rather than buffer.
const PROGRESS_CHANNEL_CAPACITY: usize = 16;

impl ProgressSink {
    /// A sink paired with the receiver that drains it.
    pub fn channel() -> (Self, tokio::sync::mpsc::Receiver<AgentProgress>) {
        let (tx, rx) = tokio::sync::mpsc::channel(PROGRESS_CHANNEL_CAPACITY);
        (Self(tx), rx)
    }

    /// Best-effort send: never blocks, and a full or closed channel is
    /// silently dropped rather than propagated as an error.
    pub fn send(&self, progress: AgentProgress) {
        let _ = self.0.try_send(progress);
    }
}

/// The result of running one check's agent in-process.
///
/// The authoritative signal is [`AgentOutcome::verdict`]: when present, the agent
/// reported via the judge tool. The remaining fields are diagnostics that
/// distinguish the *new* failure modes — an agent that hit `max_turns` without
/// reporting, a stream error, or a timeout — so execution can synthesize a clear
/// "errored" reason when no verdict arrived.
#[derive(Debug, Clone, Default)]
pub struct AgentOutcome {
    /// The verdict the agent reported via the judge tool, if it reported at all.
    pub verdict: Option<CheckReport>,
    /// Why the agent's loop stopped (human-readable), for diagnostics when no
    /// verdict was reported.
    pub stop_reason: Option<String>,
    /// How many turns the agent took (best-effort; `0` when unavailable).
    pub turns: u32,
    /// An execution-level error distinct from a check merely *failing* (stream
    /// error, agent-build error, or timeout). `None` on a clean finish.
    pub error: Option<String>,
    /// The self-contained NDJSON session trace for this one execution, when
    /// trace capture is enabled (`multi check --trace-archive`). `None` when
    /// capture is off and for executors that don't produce traces (the test
    /// fake). The execution layer moves these into the per-run
    /// [`crate::checks::trace_archive`] bundle.
    pub trace_jsonl: Option<Vec<u8>>,
    /// The allowlisted, read-only tool calls this attempt made (MULTI-1817):
    /// each `ToolStart` paired with its `ToolEnd` by `id`, kept only when the
    /// tool is on the `ReadOnlyTool` allowlist and the call finished with
    /// `is_error == false`, in `ToolStart` (call) order. Populated
    /// unconditionally — this is the frozen evidence the Jev decision engine
    /// replays; it has no non-test reader until the `jev` modules land.
    pub tool_calls: Vec<ToolCall>,
    /// Which decision engine produced this outcome (MULTI-1825): default
    /// [`DecidedBy::Agent`] — every executor except `JevExecutor` leaves this
    /// at its default, since they only ever run the agent. `JevExecutor`
    /// sets [`DecidedBy::Cached`]/[`DecidedBy::Jev`] on the well-formed
    /// [`AgentOutcome`] it synthesizes when it settles a check without
    /// running the agent at all (`turns: 0`, empty `tool_calls`, no trace —
    /// see that module's docs). Carried through
    /// [`crate::checks::execution::reconcile`] into
    /// [`crate::checks::model::CheckOutcome::decided_by`].
    pub decided_by: DecidedBy,
    /// The CoW sandbox root this attempt actually ran an agent in, if it ran
    /// one at all (MULTI-1826). Populated **unconditionally** by every
    /// executor that acquires [`AgentRunRequest::sandbox`] to run an agent
    /// (`CerseiExecutor`, and the test [`FakeExecutor`]) — `None` only for an
    /// executor that settles a check without ever acquiring the lease (the
    /// Jev decision engine's own cached/Jev-settled outcomes, which carry no
    /// captured calls to relativize in the first place).
    ///
    /// `AgentRunRequest` is moved into `CheckExecutor::run_check`, so a
    /// caller that needs the sandbox path *after* that call returns (as
    /// `JevExecutor` does, to relativize a captured call against the sandbox
    /// that produced it once the sandbox itself is already torn down) has no
    /// other way to recover it — see `crate::checks::jev::executor`'s module
    /// docs on why this is the chosen fix over acquiring the lease earlier or
    /// relativizing inside the inner executor.
    pub sandbox_root: Option<PathBuf>,
}

impl AgentOutcome {
    /// Whether the agent reported a verdict (the only authoritative signal).
    pub fn has_verdict(&self) -> bool {
        self.verdict.is_some()
    }
}

/// The abstraction over running a single check's agent.
#[async_trait]
pub trait CheckExecutor: Send + Sync {
    /// Run a single check's agent and return its verdict/outcome.
    async fn run_check(&self, req: AgentRunRequest) -> Result<AgentOutcome>;

    /// Called exactly once, after `multi check`'s whole pipeline has settled
    /// every check (MULTI-1826) — see `crate::checks::run`'s call site,
    /// right after `run_pipeline` returns and before the process's exit code
    /// is computed. Default: a no-op, so this is byte-for-byte free for
    /// every executor except `JevExecutor` (`--features jev`), which
    /// overrides it to flush its self-healing `.check-plan.toml` updates.
    /// Never touches a check's verdict or the run's exit code — both are
    /// already final by the time this runs.
    async fn finalize(&self) {}
}

/// A boxed [`CheckExecutor`] for dynamic dispatch (DI seam).
pub type BoxedExecutor = Box<dyn CheckExecutor + Send + Sync>;

/// The reporting directive for the default (in-process) executor: call the judge
/// tool exactly once. Kept separate from [`assemble_instructions`] so other
/// executors can substitute their own reporting channel.
pub fn judge_tool_directive() -> String {
    format!(
        "Carry out the check described below. When — and only when — you have reached a conclusion, \
you MUST call the `{JUDGE_TOOL}` tool EXACTLY ONCE:\n\
  - set `success` to true if the check passes, or false if it fails;\n\
  - optionally set `evidence` to a short explanation of how you concluded.\n\
Report your result ONLY through `{JUDGE_TOOL}` — not via stdout, not via a file — and do not \
call it more than once. After calling it, stop. If you finish without calling `{JUDGE_TOOL}`, \
the check is treated as a FAILURE.",
    )
}

/// Assemble the instruction text handed to an agent: standing operating
/// instructions, the executor-supplied `reporting` directive, then the check
/// prompt verbatim. (MULTI-1350, parametrized in MULTI-1367.)
///
/// The sandbox path is stated explicitly. Agents that weren't told it (pre
/// 2026-07-01 postmortem) went hunting for the repository across the host
/// filesystem — guessing paths out of the loaded CLAUDE.md — and timed out
/// inside unbounded directory walks. On a retry (`attempt > 1`) the agent is
/// also told a previous attempt went unreported, so the new trajectory has a
/// reason to differ from the failed one.
///
/// Since MULTI-1834 the sandbox spans the requirement's whole repository root
/// rather than just the directory `multi check` was scanned from, so the
/// instructions also state `declared_in` — the declaring file's path relative
/// to that root — to preserve the implicit scoping a smaller sandbox used to
/// provide for free.
pub fn assemble_instructions(
    check: &Check,
    reporting: &str,
    working_dir: &Path,
    declared_in: &Path,
    attempt: u32,
) -> String {
    let retry_note = if attempt > 1 {
        format!(
            "\nNOTE: this is attempt {attempt} for this check; a previous attempt finished \
without reporting a verdict (it may have timed out while exploring). Stay inside the \
working directory and report as soon as the evidence supports a conclusion.\n"
        )
    } else {
        String::new()
    };
    format!(
        "You are validating a single requirement for the MultiTool Checks tool.\n\
Your working directory is `{working_dir}` — a sandboxed, throwaway copy of the user's \
repository that you may inspect freely. Every file relevant to this check lives under \
that path: do not read or search outside it. Tool calls that take an optional `path` \
default to it when omitted.\n\
This requirement is declared in `{declared_in}`.\n\
{retry_note}\
\n\
{reporting}\n\
\n\
--- CHECK: {title} ---\n\
{prompt}\n",
        working_dir = working_dir.display(),
        declared_in = declared_in.display(),
        title = check.title,
        prompt = check.prompt,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(CheckExecutor);

    fn check() -> Check {
        Check {
            title: "No yellow".into(),
            prompt: "scan for yellow text".into(),
        }
    }

    #[test]
    fn instructions_embed_prompt_and_demand_single_report() {
        let text = assemble_instructions(
            &check(),
            &judge_tool_directive(),
            Path::new("/tmp/sandbox-copy"),
            Path::new("CHECKS.md"),
            1,
        );
        assert!(text.contains("scan for yellow text"));
        assert!(text.contains(JUDGE_TOOL));
        assert!(text.contains("EXACTLY ONCE"));
        assert!(text.contains("No yellow"));
    }

    #[test]
    fn instructions_state_the_working_directory_path() {
        let text = assemble_instructions(
            &check(),
            &judge_tool_directive(),
            Path::new("/tmp/sandbox-copy"),
            Path::new("CHECKS.md"),
            1,
        );
        assert!(text.contains("`/tmp/sandbox-copy`"));
        assert!(text.contains("do not read or search outside it"));
        // First attempts carry no retry note.
        assert!(!text.contains("previous attempt"));
    }

    #[test]
    fn retries_carry_a_note_about_the_unreported_attempt() {
        let text = assemble_instructions(
            &check(),
            &judge_tool_directive(),
            Path::new("/tmp/sandbox-copy"),
            Path::new("CHECKS.md"),
            2,
        );
        assert!(text.contains("attempt 2"));
        assert!(text.contains("previous attempt finished without reporting"));
    }

    /// MULTI-1834 acceptance: the declaring file's root-relative path appears
    /// in the assembled instructions, so the agent retains the scoping a
    /// smaller, per-scan-directory sandbox used to provide for free.
    #[test]
    fn instructions_state_where_the_requirement_was_declared() {
        let text = assemble_instructions(
            &check(),
            &judge_tool_directive(),
            Path::new("/tmp/sandbox-copy"),
            Path::new("services/keystore/CHECKS.md"),
            1,
        );
        assert!(text.contains("This requirement is declared in `services/keystore/CHECKS.md`"));
    }
}
