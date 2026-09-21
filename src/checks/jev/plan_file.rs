//! The `.check-plan.toml` schema and store (MULTI-1820).
//!
//! A plan freezes, per check, the read-only tool calls needed to verify it —
//! captured once by [`crate::checks::executor::tool_capture`] at plan time —
//! plus a checksum of each call's replay output. `multi plan` (MULTI-1824)
//! writes one `.check-plan.toml` beside every `CHECKS.md`; `multi check`
//! (MULTI-1822/1825) replays the frozen calls, compares checksums, and only
//! falls back to a full reasoning-agent run when the evidence changed. Plans
//! are meant to be committed, so every value here is portable: no absolute
//! paths, no host-specific data, and deterministic output so a re-plan's diff
//! is reviewable.
//!
//! ## Why this module hand-renders TOML instead of `toml::to_string`
//!
//! The `toml` crate's derive-driven serializer *always* promotes a
//! struct/map-shaped field to its own `[section]` (or `[[array-of-tables]]`)
//! header, even when that field sits inside an array-of-tables element —
//! confirmed empirically: a `Vec<Call>` field's own nested `input: toml::Value`
//! table still became a separate `[requirement.check.call.input]` section
//! rather than the ticket's required `input = { file_path = "..." }` inline
//! form. There is no `#[serde(...)]` attribute to opt a single field out of
//! that promotion. `toml::Value`'s own `Display` impl (used by
//! [`toml::Value::try_from`]), by contrast, always renders a `Table` value
//! inline (`{ k = v, ... }`) — it has no section/header concept at all, since
//! it renders one *value*, not a document. So [`PlanFile::to_toml_string`]
//! walks the (small, fixed-depth) schema itself, writing `[[...]]` headers by
//! hand and every `key = value` line via [`toml::Value::try_from`] (for typed
//! Rust values) or the hand-rolled [`json_to_toml`] (for a call's `input`).
//! Reading a plan back needs none of this: TOML's `[section]` and inline
//! `{...}` forms parse to the identical value tree, so ordinary
//! `#[derive(Deserialize)]` on every type here reads either form the same way
//! (verified: parsing this module's own hand-rendered inline output back
//! through the derived `Deserialize` impls round-trips correctly).
//!
//! ## Null handling
//!
//! A captured tool call's `input` is a [`serde_json::Value`] object; TOML has
//! no `null`, and the `toml` crate errors on one. An agent's tool call
//! represents an omitted optional argument as an explicit JSON `null` (e.g.
//! `{"file_path": "...", "limit": null}`) exactly as often as it omits the key
//! entirely, and the two are equivalent for these tools — so [`relativize`]
//! strips every null-valued key (recursively) before a call's input is ever
//! stored, rather than rejecting the plan or inventing a TOML-safe null
//! encoding.
//!
//! ## Determinism
//!
//! Deterministic output is a property of [`PlanStore::write`], not of
//! whatever order a caller happens to assemble a [`PlanFile`] in —
//! MULTI-1824's planner collects results from bounded-concurrency tasks, so
//! assembly order will vary run to run. `write` stable-sorts a cloned plan's
//! `requirements` by `(source, ordinal)` and each requirement's `checks` by
//! `ordinal` before rendering; [`PlanFile::to_toml_string`] itself renders
//! whatever order it's handed, verbatim. A call's order within its check is
//! left untouched — replay order is meaningful, not an artifact to sort away
//! — only exact duplicates are removed (see
//! [`dedup_calls_preserving_order`]).
//!
//! `serde_json`'s `preserve_order` feature is enabled in this workspace
//! (transitively — verified with `cargo metadata`), so [`serde_json::Value`]
//! objects iterate in *insertion* order, not sorted order. A captured tool
//! call's key order reflects whatever order the calling agent happened to
//! supply arguments in, which is not a property we want leaking into a
//! committed plan's diffs. [`json_to_toml`] therefore sorts object keys
//! explicitly. Integer-vs-float is preserved through the `toml::Value`
//! round trip via [`serde_json::Number`]'s own `as_i64`/`as_u64`/`as_f64`
//! accessors (safe regardless of this crate's `arbitrary_precision`
//! `serde_json` feature, which is also transitively enabled and would
//! otherwise make a naive generic `Serialize` of a `serde_json::Value`
//! produce a bogus one-key wrapper map for every number — see
//! `crate::checks::jev::types`'s `Answer` deserializer for the same
//! `arbitrary_precision` gotcha on the read side).

use std::path::{Component, Path, PathBuf};

use miette::Diagnostic;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::checks::executor::ReadOnlyTool;

/// The only schema version this module understands. [`PlanStore::load`]
/// rejects any other value with [`PlanError::UnknownVersion`] rather than
/// attempting to parse a plan it can't interpret.
pub const CURRENT_VERSION: u32 = 1;

/// The plan file's name, a fixed sibling of the `CHECKS.md`/`CHECKS.toml` it
/// covers — never configurable, so a plan is always found the same way
/// regardless of invocation directory (see the module docs on portability).
pub const PLAN_FILE_NAME: &str = ".check-plan.toml";

/// Seed for every xxHash64 checksum this module produces, fixed so checksums
/// are stable across runs and machines (matching `crates/multi-core`'s
/// xxHash32 convention — see `crates/multi-core/src/hashing/mod.rs`).
const XXH64_SEED: u64 = 0;

// ---------------------------------------------------------------------------
// Checksum helpers (reused by MULTI-1822's replay and MULTI-1824's planner).
// ---------------------------------------------------------------------------

/// Render an xxHash64 digest of `data` as fixed-width, lowercase hex (16
/// characters, zero-padded — `{:016x}`, not `{:x}`) so every checksum in a
/// plan is the same length and diffs stay byte-aligned regardless of leading
/// zero bytes in the digest.
pub fn xxh64_hex(data: &[u8]) -> String {
    format!("{:016x}", twox_hash::XxHash64::oneshot(XXH64_SEED, data))
}

/// Hash a check's `title` + `prompt` into `prompt_xxh64`: [`PlanFile::lookup`]
/// treats a plan whose stored hash no longer matches this as not found, since
/// the check itself changed since the plan was written and the frozen
/// evidence no longer applies to it.
///
/// `title` is length-prefixed (decimal byte length, then a `\0` terminator,
/// then `title`'s own bytes) before `prompt` is appended, so the split
/// between the two is driven by an explicit count rather than a scanned
/// separator — unambiguous even if `title` or `prompt` themselves contain
/// `\0` or any other byte. Plain concatenation would let e.g. `("a", "bc")`
/// and `("ab", "c")` collide; this construction can't.
pub fn prompt_xxh64(title: &str, prompt: &str) -> String {
    let mut buf = Vec::with_capacity(title.len() + prompt.len() + 12);
    buf.extend_from_slice(title.len().to_string().as_bytes());
    buf.push(0);
    buf.extend_from_slice(title.as_bytes());
    buf.extend_from_slice(prompt.as_bytes());
    xxh64_hex(&buf)
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// The parsed/in-memory form of a `.check-plan.toml` file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PlanFile {
    pub version: u32,
    #[serde(rename = "requirement", default)]
    pub requirements: Vec<PlanRequirement>,
}

impl PlanFile {
    /// Build a fresh, [`CURRENT_VERSION`] plan from already-assembled
    /// requirements.
    pub fn new(requirements: Vec<PlanRequirement>) -> Self {
        Self {
            version: CURRENT_VERSION,
            requirements,
        }
    }

    /// Find the frozen check identified by `(source, req_ordinal,
    /// check_ordinal)`, but only when its stored `prompt_xxh64` still matches
    /// `prompt_xxh64` — a mismatch means the check's title/prompt changed
    /// since the plan was written, so its frozen evidence no longer applies
    /// and callers must treat it exactly like a missing plan.
    pub fn lookup(
        &self,
        source: &str,
        req_ordinal: u32,
        check_ordinal: u32,
        prompt_xxh64: &str,
    ) -> Option<&PlanCheck> {
        let requirement = self
            .requirements
            .iter()
            .find(|r| r.source == source && r.ordinal == req_ordinal)?;
        let check = requirement
            .checks
            .iter()
            .find(|c| c.ordinal == check_ordinal)?;
        (check.prompt_xxh64 == prompt_xxh64).then_some(check)
    }

    /// Hand-render this plan as TOML text (see the module docs for why this
    /// isn't `toml::to_string`). `[[requirement]]` blocks are emitted in
    /// `self.requirements`' order, verbatim — this function does not sort
    /// anything; [`PlanStore::write`] establishes the canonical, deterministic
    /// order on a cloned plan before calling this.
    fn to_toml_string(&self) -> Result<String, PlanError> {
        let mut out = String::new();
        write_kv(&mut out, "version", &self.version)?;
        for requirement in &self.requirements {
            requirement.render(&mut out)?;
        }
        Ok(out)
    }
}

/// One `[[requirement]]` block: a single non-functional requirement declared
/// by a `CHECKS.md` (or, later, `CHECKS.toml` — MULTI-1831) file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PlanRequirement {
    pub title: String,
    /// The declaring file's name (e.g. `"CHECKS.md"`) — not a path. A plan
    /// always sits beside the file(s) it covers, so this is only ever used to
    /// disambiguate which declaring file a requirement came from once a
    /// directory can hold both `CHECKS.md` and `CHECKS.toml` (MULTI-1831).
    pub source: String,
    /// This requirement's position within `source`. Titles are not unique, so
    /// `(source, ordinal)` — not `title` — identifies a requirement.
    pub ordinal: u32,
    #[serde(rename = "check", default)]
    pub checks: Vec<PlanCheck>,
}

impl PlanRequirement {
    fn render(&self, out: &mut String) -> Result<(), PlanError> {
        out.push_str("\n[[requirement]]\n");
        write_kv(out, "title", &self.title)?;
        write_kv(out, "source", &self.source)?;
        write_kv(out, "ordinal", &self.ordinal)?;
        for check in &self.checks {
            check.render(out)?;
        }
        Ok(())
    }
}

/// One `[[requirement.check]]` block: a single check's frozen evidence and
/// plan-time verdict.
///
/// Deserialized via [`RawPlanCheck`] + `TryFrom`, not derived directly: the
/// wire schema's `decider`/`agent_reason` pairing (`decider = "jev"` must not
/// carry an `agent_reason`; `decider = "agent"` must carry one) is a
/// cross-field constraint `#[derive(Deserialize)]` can't express, and folding
/// `agent_reason` into [`Decider::Agent`] makes the invalid pairing
/// unrepresentable in memory too — see [`Decider`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlanCheck {
    pub title: String,
    /// This check's position within its requirement. Titles are not unique
    /// (a check may inherit its requirement's title when anonymous), so
    /// `(requirement, ordinal)` — not `title` — identifies a check.
    pub ordinal: u32,
    /// Hash of `title` + the check's prompt (see [`prompt_xxh64`]); a
    /// mismatch invalidates this entry — see [`PlanFile::lookup`].
    pub prompt_xxh64: String,
    /// Which decision engine settled this check the last time it ran (and,
    /// for [`Decider::Agent`], why).
    pub decider: Decider,
    /// The reasoning agent's verdict at plan time (`true` = satisfied).
    pub verdict: bool,
    /// The agent's optional explanation of `verdict`.
    pub evidence: Option<String>,
    /// The Jev calibration recorded while establishing `decider`, if Jev was
    /// consulted at all (see [`JevCalibration`]).
    pub jev: Option<JevCalibration>,
    pub calls: Vec<PlanCall>,
}

impl PlanCheck {
    fn render(&self, out: &mut String) -> Result<(), PlanError> {
        out.push_str("\n[[requirement.check]]\n");
        write_kv(out, "title", &self.title)?;
        write_kv(out, "ordinal", &self.ordinal)?;
        write_kv(out, "prompt_xxh64", &self.prompt_xxh64)?;
        write_kv(out, "decider", self.decider.wire_str())?;
        write_kv(out, "verdict", &self.verdict)?;
        if let Some(evidence) = &self.evidence {
            write_kv(out, "evidence", evidence)?;
        }
        if let Some(jev) = &self.jev {
            write_kv(out, "jev", jev)?;
        }
        if let Decider::Agent(reason) = self.decider {
            write_kv(out, "agent_reason", &reason)?;
        }
        for call in &self.calls {
            call.render(out)?;
        }
        Ok(())
    }
}

/// The literal on-disk shape of a `[[requirement.check]]` block's
/// `decider`/`agent_reason` fields plus everything else — used only to parse
/// a check before [`PlanCheck::try_from`] validates the `decider`/
/// `agent_reason` pairing (see [`PlanCheck`]) and converts to the
/// enforced-shape type.
#[derive(Debug, Deserialize)]
struct RawPlanCheck {
    title: String,
    ordinal: u32,
    prompt_xxh64: String,
    decider: DeciderTag,
    verdict: bool,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    jev: Option<JevCalibration>,
    #[serde(default)]
    agent_reason: Option<AgentReason>,
    #[serde(rename = "call", default)]
    call: Vec<PlanCall>,
}

impl TryFrom<RawPlanCheck> for PlanCheck {
    type Error = String;

    fn try_from(raw: RawPlanCheck) -> Result<Self, Self::Error> {
        let decider = match (raw.decider, raw.agent_reason) {
            (DeciderTag::Jev, None) => Decider::Jev,
            (DeciderTag::Jev, Some(_)) => {
                return Err(
                    "a check with `decider = \"jev\"` must not carry an `agent_reason`".to_string(),
                );
            }
            (DeciderTag::Agent, Some(reason)) => Decider::Agent(reason),
            (DeciderTag::Agent, None) => {
                return Err(
                    "a check with `decider = \"agent\"` is missing `agent_reason`".to_string(),
                );
            }
        };
        Ok(PlanCheck {
            title: raw.title,
            ordinal: raw.ordinal,
            prompt_xxh64: raw.prompt_xxh64,
            decider,
            verdict: raw.verdict,
            evidence: raw.evidence,
            jev: raw.jev,
            calls: raw.call,
        })
    }
}

impl<'de> Deserialize<'de> for PlanCheck {
    /// Deserializes via [`RawPlanCheck`] — see [`PlanCheck`]'s docs on why a
    /// plain derive can't express the `decider`/`agent_reason` constraint.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let raw = RawPlanCheck::deserialize(deserializer)?;
        PlanCheck::try_from(raw).map_err(D::Error::custom)
    }
}

/// Which decision engine settled a check — and, for [`Decider::Agent`], why
/// (PRD objective #3: Jev only decides checks it has demonstrated it can
/// decide; the ticket's closed set of reasons live on [`AgentReason`], and
/// MULTI-1824 always records one). Folding the reason into this variant
/// (rather than a separate `Option<AgentReason>` field on [`PlanCheck`])
/// makes `decider = "jev"` + a reason, or `decider = "agent"` + no reason,
/// unrepresentable in memory — not just rejected at parse time (see
/// [`RawPlanCheck`]'s `TryFrom` for the on-disk-side enforcement of the same
/// constraint).
///
/// Rendered on the wire as two separate keys (`decider = "jev" | "agent"`,
/// and `agent_reason = "..."` only for `Agent`) — see [`Decider::wire_str`]
/// and [`PlanCheck::render`] — not as a single tagged value, so this type
/// intentionally isn't `Serialize`/`Deserialize` itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decider {
    Jev,
    Agent(AgentReason),
}

impl Decider {
    /// The `decider` key's wire value alone (`agent_reason`, when present,
    /// is a separate key — see [`PlanCheck::render`]).
    fn wire_str(self) -> &'static str {
        match self {
            Decider::Jev => "jev",
            Decider::Agent(_) => "agent",
        }
    }
}

/// The wire-level `decider` tag alone (`"jev"` | `"agent"`), used only by
/// [`RawPlanCheck`] before it's paired with `agent_reason` to build the
/// enforced-shape [`Decider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DeciderTag {
    Jev,
    Agent,
}

/// Why a check fell back to the reasoning agent instead of being settled by
/// Jev alone — see [`Decider::Agent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentReason {
    /// Jev's verdict disagreed with the reasoning agent's at plan time.
    #[serde(rename = "jev disagreed")]
    JevDisagreed,
    /// Jev's `noul` fell below the configured confidence threshold.
    #[serde(rename = "jev uncertain")]
    JevUncertain,
    /// The empty-evidence negative control did not reject as expected.
    #[serde(rename = "control failed")]
    ControlFailed,
    /// Deciding this check would exceed the configured cost/latency budget.
    #[serde(rename = "over budget")]
    OverBudget,
    /// The frozen plan has no calls to replay for this check.
    #[serde(rename = "no tool calls")]
    NoToolCalls,
    /// A call's replay output hit the tool's result cap and can't be trusted.
    #[serde(rename = "truncated discovery")]
    TruncatedDiscovery,
}

/// The Jev calibration recorded for a check: its own verdict noul, a
/// negative-control noul (run against empty evidence — a question Jev is
/// biased toward answering "yes" must not pass this), and the record-only
/// Choice `reading`. Rendered as one inline table (`jev = { ... }`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevCalibration {
    /// The concrete Jev model that answered (e.g. `"jev-1.13.0"`), which may
    /// differ from a requested alias.
    pub model: String,
    pub noul: f64,
    pub control_noul: f64,
    pub reading: Reading,
}

/// The record-only Choice reading paired with a [`JevCalibration`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reading {
    Satisfied,
    Violated,
    Insufficient,
}

/// One `[[requirement.check.call]]` block: a single frozen, read-only tool
/// call plus the checksum(s) needed to detect when replaying it would
/// produce different output.
///
/// Modeled as an enum (rather than one struct with several `Option`
/// checksum fields) so the tool/checksum-shape pairing the ticket specifies —
/// `Read`/`Glob` carry one `xxh64`; `Grep` carries `files_xxh64` +
/// `lines_xxh64` (discovery half, content half); a truncated call carries
/// neither — can't be constructed any other way. [`PlanCall::deserialize`]
/// enforces the same pairing when reading a plan back (see [`RawPlanCall`]),
/// so a hand-edited or corrupted file with e.g. a `Read` call carrying
/// `files_xxh64` is rejected as a parse error rather than silently accepted.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanCall {
    /// `input.file_path` was read; `xxh64` checksums the normalized replay
    /// output.
    Read { input: Value, xxh64: String },
    /// `input.pattern`/`input.path` were globbed; `xxh64` checksums the
    /// normalized replay output (the sorted match list).
    Glob { input: Value, xxh64: String },
    /// `input.pattern`/`input.path` were searched; `files_xxh64` checksums
    /// the sorted set of matched files (the discovery half) and
    /// `lines_xxh64` checksums `path:content` with line numbers stripped
    /// (the content half).
    Grep {
        input: Value,
        files_xxh64: String,
        lines_xxh64: String,
    },
    /// The call hit the tool's result cap: its output isn't reproducible, is
    /// excluded from freshness checksumming, and forces `decider = "agent"`
    /// for the owning check (see [`AgentReason::TruncatedDiscovery`]).
    Truncated { tool: ReadOnlyTool, input: Value },
}

impl PlanCall {
    fn render(&self, out: &mut String) -> Result<(), PlanError> {
        out.push_str("\n[[requirement.check.call]]\n");
        match self {
            PlanCall::Read { input, xxh64 } => {
                write_kv(out, "tool", &ReadOnlyTool::Read)?;
                write_value(out, "input", &json_to_toml(input)?);
                write_kv(out, "xxh64", xxh64)?;
            }
            PlanCall::Glob { input, xxh64 } => {
                write_kv(out, "tool", &ReadOnlyTool::Glob)?;
                write_value(out, "input", &json_to_toml(input)?);
                write_kv(out, "xxh64", xxh64)?;
            }
            PlanCall::Grep {
                input,
                files_xxh64,
                lines_xxh64,
            } => {
                write_kv(out, "tool", &ReadOnlyTool::Grep)?;
                write_value(out, "input", &json_to_toml(input)?);
                write_kv(out, "files_xxh64", files_xxh64)?;
                write_kv(out, "lines_xxh64", lines_xxh64)?;
            }
            PlanCall::Truncated { tool, input } => {
                write_kv(out, "tool", tool)?;
                write_value(out, "input", &json_to_toml(input)?);
                write_kv(out, "truncated", &true)?;
            }
        }
        Ok(())
    }
}

/// The literal on-disk shape of a `[[requirement.check.call]]` block — every
/// possible field as an `Option`/`bool` — used only to parse a call before
/// [`PlanCall::try_from`] validates which combination of checksum fields is
/// actually present and converts to the enforced-shape [`PlanCall`].
#[derive(Debug, Deserialize)]
struct RawPlanCall {
    tool: ReadOnlyTool,
    input: toml::Value,
    #[serde(default)]
    xxh64: Option<String>,
    #[serde(default)]
    files_xxh64: Option<String>,
    #[serde(default)]
    lines_xxh64: Option<String>,
    #[serde(default)]
    truncated: bool,
}

impl TryFrom<RawPlanCall> for PlanCall {
    type Error = String;

    fn try_from(raw: RawPlanCall) -> Result<Self, Self::Error> {
        let input = toml_to_json(&raw.input);
        if raw.truncated {
            if raw.xxh64.is_some() || raw.files_xxh64.is_some() || raw.lines_xxh64.is_some() {
                return Err(format!(
                    "a truncated call for tool `{:?}` must not also carry a checksum",
                    raw.tool
                ));
            }
            return Ok(PlanCall::Truncated {
                tool: raw.tool,
                input,
            });
        }

        match raw.tool {
            ReadOnlyTool::Read | ReadOnlyTool::Glob => {
                if raw.files_xxh64.is_some() || raw.lines_xxh64.is_some() {
                    return Err(format!(
                        "a `{:?}` call must not carry Grep's `files_xxh64`/`lines_xxh64`",
                        raw.tool
                    ));
                }
                let xxh64 = raw
                    .xxh64
                    .ok_or_else(|| format!("a `{:?}` call is missing `xxh64`", raw.tool))?;
                Ok(match raw.tool {
                    ReadOnlyTool::Read => PlanCall::Read { input, xxh64 },
                    ReadOnlyTool::Glob => PlanCall::Glob { input, xxh64 },
                    ReadOnlyTool::Grep => unreachable!("matched above"),
                })
            }
            ReadOnlyTool::Grep => {
                if raw.xxh64.is_some() {
                    return Err(
                        "a `Grep` call must not carry a single `xxh64`; use `files_xxh64`/`lines_xxh64`"
                            .to_string(),
                    );
                }
                let files_xxh64 = raw
                    .files_xxh64
                    .ok_or_else(|| "a `Grep` call is missing `files_xxh64`".to_string())?;
                let lines_xxh64 = raw
                    .lines_xxh64
                    .ok_or_else(|| "a `Grep` call is missing `lines_xxh64`".to_string())?;
                Ok(PlanCall::Grep {
                    input,
                    files_xxh64,
                    lines_xxh64,
                })
            }
        }
    }
}

impl<'de> Deserialize<'de> for PlanCall {
    /// Deserializes via [`RawPlanCall`] rather than a `#[serde(untagged)]`
    /// enum: an untagged enum tries each variant in turn and reports only the
    /// last variant's (usually unhelpful) error on total failure, whereas
    /// this reports exactly which required checksum field is missing or
    /// which tool/checksum combination is invalid.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let raw = RawPlanCall::deserialize(deserializer)?;
        PlanCall::try_from(raw).map_err(D::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// TOML value rendering (write path — see the module docs)
// ---------------------------------------------------------------------------

/// Render `value` as a bare TOML value via [`toml::Value::try_from`] (which,
/// unlike `toml::to_string`, never promotes a table-shaped value to its own
/// `[section]` — see the module docs) and write it as `key = <value>\n`.
fn write_kv<T>(out: &mut String, key: &str, value: &T) -> Result<(), PlanError>
where
    T: Serialize + ?Sized,
{
    let rendered = toml::Value::try_from(value).map_err(PlanError::Serialize)?;
    write_value(out, key, &rendered);
    Ok(())
}

/// Write an already-built [`toml::Value`] as `key = <value>\n`.
fn write_value(out: &mut String, key: &str, value: &toml::Value) {
    out.push_str(key);
    out.push_str(" = ");
    out.push_str(&value.to_string());
    out.push('\n');
}

/// Convert a captured tool call's JSON `input` into a [`toml::Value`],
/// recursively, sorting object keys so the emitted TOML is deterministic
/// regardless of the agent's original argument order (see the module docs).
/// A `Null` is rejected — [`relativize`] is expected to have already stripped
/// every null-valued key before a call's input is stored.
fn json_to_toml(value: &Value) -> Result<toml::Value, PlanError> {
    match value {
        Value::Null => Err(PlanError::NullValue),
        Value::Bool(b) => Ok(toml::Value::Boolean(*b)),
        Value::Number(n) => json_number_to_toml(n),
        Value::String(s) => Ok(toml::Value::String(s.clone())),
        Value::Array(items) => Ok(toml::Value::Array(
            items.iter().map(json_to_toml).collect::<Result<_, _>>()?,
        )),
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            let mut table = toml::Table::new();
            for (key, value) in entries {
                table.insert(key.clone(), json_to_toml(value)?);
            }
            Ok(toml::Value::Table(table))
        }
    }
}

/// `serde_json::Number` carries its int/float-ness even with this workspace's
/// `arbitrary_precision` feature enabled (see the module docs) — its
/// `as_i64`/`as_u64`/`as_f64` accessors are unaffected by that feature and are
/// exactly what lets this stay a plain match rather than a
/// `Serialize`-through-a-generic-serializer round trip.
fn json_number_to_toml(n: &serde_json::Number) -> Result<toml::Value, PlanError> {
    if let Some(i) = n.as_i64() {
        Ok(toml::Value::Integer(i))
    } else if let Some(u) = n.as_u64() {
        i64::try_from(u)
            .map(toml::Value::Integer)
            .map_err(|_| PlanError::NumberOutOfRange(n.to_string()))
    } else if let Some(f) = n.as_f64() {
        Ok(toml::Value::Float(f))
    } else {
        Err(PlanError::NumberOutOfRange(n.to_string()))
    }
}

/// The inverse of [`json_to_toml`], used when reading a call's `input` back
/// off disk. Infallible: every [`toml::Value`] shape has a direct JSON
/// equivalent (a `Datetime` — never produced by [`json_to_toml`], but
/// reachable if someone hand-edits a plan — degrades to its string form
/// rather than failing the whole plan load).
fn toml_to_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number((*i).into()),
        toml::Value::Float(f) => {
            serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number)
        }
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

// ---------------------------------------------------------------------------
// Path normalization
// ---------------------------------------------------------------------------

/// Normalize a captured tool call's path-bearing input keys
/// (`Read.file_path`; `Grep.path`/`Glob.path`) to be relative to
/// `sandbox_root`, and strip every null-valued key (recursively — see the
/// module docs) so the result is TOML-safe.
///
/// An omitted `Grep`/`Glob` `path` (the tool defaults it to the working
/// directory) is written out explicitly as `"."`, so a call's scope is frozen
/// in the plan rather than implied by replay context. `Read.file_path` is
/// required by the tool itself, so it is never defaulted here.
///
/// A `Glob` `pattern` may itself be an absolute path (e.g.
/// `/abs/dir/**/*.rs`, as opposed to a bare pattern like `**/*.rs`); when it
/// is, it's relativized the same way `path` is — rejected if it escapes
/// `sandbox_root`, left as a relative pattern string otherwise.
///
/// Rejects any path that escapes `sandbox_root` — including via `..`
/// components and via an absolute path outside the root — with
/// [`PlanError::PathEscape`]. `sandbox_root` and macOS's `/var`↔`/private/var`
/// symlink spelling of the same directory are treated as equal (see
/// [`normalize_macos_private_prefix`]) without touching the filesystem, so
/// this works even when the path no longer exists.
pub fn relativize(
    tool: ReadOnlyTool,
    input: &Value,
    sandbox_root: &Path,
) -> Result<Value, PlanError> {
    let mut input = input.clone();
    strip_null_values(&mut input);

    let Some(obj) = input.as_object_mut() else {
        return Err(PlanError::MalformedInput {
            tool,
            reason: "tool-call input must be a JSON object".to_string(),
        });
    };

    let path_key = match tool {
        ReadOnlyTool::Read => "file_path",
        ReadOnlyTool::Grep | ReadOnlyTool::Glob => "path",
    };

    match obj.remove(path_key) {
        Some(Value::String(raw)) => {
            let relative = relativize_path(&raw, sandbox_root)?;
            obj.insert(path_key.to_string(), Value::String(relative));
        }
        Some(_) => {
            return Err(PlanError::MalformedInput {
                tool,
                reason: format!("`{path_key}` must be a string"),
            });
        }
        None => {
            if matches!(tool, ReadOnlyTool::Grep | ReadOnlyTool::Glob) {
                obj.insert(path_key.to_string(), Value::String(".".to_string()));
            }
        }
    }

    if tool == ReadOnlyTool::Glob
        && let Some(Value::String(pattern)) = obj.get("pattern").cloned()
        && Path::new(&pattern).is_absolute()
    {
        let relative = relativize_path(&pattern, sandbox_root)?;
        obj.insert("pattern".to_string(), Value::String(relative));
    }

    Ok(input)
}

/// Recursively strip null-valued object keys (see [`relativize`]'s module
/// docs on why an explicit JSON `null` and an omitted key are treated the
/// same). Array elements are recursed into but never removed for being null —
/// no tool input in this allowlist has array-of-nullable-values shape today,
/// and removing an array element would change the array's meaning in a way
/// removing an object key does not.
fn strip_null_values(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            for v in map.values_mut() {
                strip_null_values(v);
            }
        }
        Value::Array(items) => {
            for v in items.iter_mut() {
                strip_null_values(v);
            }
        }
        _ => {}
    }
}

/// Relativize a single captured path/pattern string against `sandbox_root`;
/// see [`relativize`] for the full contract.
fn relativize_path(raw: &str, sandbox_root: &Path) -> Result<String, PlanError> {
    let raw_path = Path::new(raw);
    let absolute = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        sandbox_root.join(raw_path)
    };

    let normalized = normalize_macos_private_prefix(&normalize_lexical(&absolute));
    let root_normalized = normalize_macos_private_prefix(&normalize_lexical(sandbox_root));

    let relative =
        normalized
            .strip_prefix(&root_normalized)
            .map_err(|_| PlanError::PathEscape {
                path: raw.to_string(),
                root: sandbox_root.to_path_buf(),
            })?;

    // `normalize_lexical` already resolves every `..`/`.` component, so a
    // `ParentDir` surviving `strip_prefix` would mean the two normalized
    // paths were inconsistent (e.g. `sandbox_root` itself contained an
    // unresolvable `..`) rather than a legitimate relative path; treat that
    // defensively as an escape too.
    if relative
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(PlanError::PathEscape {
            path: raw.to_string(),
            root: sandbox_root.to_path_buf(),
        });
    }

    if relative.as_os_str().is_empty() {
        return Ok(".".to_string());
    }
    Ok(join_forward_slash(relative))
}

/// Join `path`'s components with `/`, regardless of the host platform's own
/// separator. A plan is committed and portable across machines (see the
/// module docs), so a stored path must not depend on
/// `std::path::MAIN_SEPARATOR` — `Path::to_string_lossy` would otherwise emit
/// `\`-joined paths on a platform where that's the native separator.
fn join_forward_slash(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Lexically normalize `path`: resolve `.`/`..` components without touching
/// the filesystem (no symlink resolution — see
/// [`normalize_macos_private_prefix`] for the one filesystem-adjacent quirk
/// this module does handle). `..` at the root is dropped (POSIX: `/..` ==
/// `/`); a leading `..` on a path with no root to cancel against (only
/// reachable if a caller passes a relative `sandbox_root`) is kept as-is —
/// callers detect that surviving case via a `ParentDir` component after
/// stripping a prefix.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(result.components().next_back(), Some(Component::Normal(_))) {
                    result.pop();
                } else if result.components().next().is_none() {
                    // Nothing accumulated yet: a relative path's leading
                    // `..`, with nothing to cancel it against — keep it.
                    result.push(component);
                }
                // Otherwise `result` is exactly the root (`/..` == `/`):
                // drop the `..`.
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// macOS symlinks several top-level directories into `/private`
/// (`/tmp`→`/private/tmp`, `/var`→`/private/var`, `/etc`→`/private/etc`).
/// Some tools canonicalize paths (following the symlink) and some don't, so
/// the same sandbox root can arrive spelled either way — most commonly
/// `$TMPDIR`-derived sandbox roots under `/var/folders/...` vs. a
/// `std::fs::canonicalize`d form under `/private/var/folders/...`. Strip a
/// leading `/private` from one of these three so both spellings compare
/// equal, purely lexically (no filesystem access, so this works even when
/// the path no longer exists).
fn normalize_macos_private_prefix(path: &Path) -> PathBuf {
    const ALIASED: &[&str] = &["tmp", "var", "etc"];

    let components: Vec<Component> = path.components().collect();
    let is_private_alias = matches!(components.first(), Some(Component::RootDir))
        && matches!(components.get(1), Some(Component::Normal(n)) if *n == "private")
        && matches!(
            components.get(2),
            Some(Component::Normal(n)) if n.to_str().is_some_and(|s| ALIASED.contains(&s))
        );

    if !is_private_alias {
        return path.to_path_buf();
    }

    // Rebuild as `/<second>/...`, dropping the leading `/private`.
    let mut rebuilt = PathBuf::from(Component::RootDir.as_os_str());
    for component in &components[2..] {
        rebuilt.push(component.as_os_str());
    }
    rebuilt
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// A lightweight probe used to read `version` before committing to a full
/// [`PlanFile`] parse — see [`PlanStore::load`].
#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

/// Loads, looks up in, and atomically writes `.check-plan.toml` files.
pub struct PlanStore;

impl PlanStore {
    /// Load the plan beside `dir` (i.e. `dir/.check-plan.toml`).
    ///
    /// Returns `Ok(None)` when no plan file exists yet — a cold plan is not
    /// an error, just nothing to reuse. Returns [`PlanError::UnknownVersion`]
    /// when the file parses far enough to read its `version` but that version
    /// isn't [`CURRENT_VERSION`], and [`PlanError::Parse`] when the file
    /// isn't valid TOML (or doesn't even have a readable `version`) —
    /// checked in that order, so a version mismatch is reported precisely
    /// rather than folded into a generic parse failure.
    pub fn load(dir: &Path) -> Result<Option<PlanFile>, PlanError> {
        let path = dir.join(PLAN_FILE_NAME);
        let contents = match std::fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(PlanError::Io { path, source }),
        };

        let probe: VersionProbe = toml::from_str(&contents).map_err(|source| PlanError::Parse {
            path: path.clone(),
            source,
        })?;
        if probe.version != CURRENT_VERSION {
            return Err(PlanError::UnknownVersion {
                path,
                version: probe.version,
                expected: CURRENT_VERSION,
            });
        }

        let plan: PlanFile = toml::from_str(&contents).map_err(|source| PlanError::Parse {
            path: path.clone(),
            source,
        })?;
        Ok(Some(plan))
    }

    /// Write `plan` beside `dir` (i.e. `dir/.check-plan.toml`), atomically:
    /// rendered to a tempfile created in `dir` itself (so the final rename is
    /// same-filesystem, hence atomic) and renamed into place, so a reader
    /// never observes a partially-written plan and a crash mid-write leaves
    /// the previous plan (or none) intact.
    ///
    /// Output is deterministic regardless of the order `plan.requirements`
    /// (and each requirement's `checks`) arrive in — a cloned copy is
    /// stable-sorted first: requirements by `(source, ordinal)`, checks by
    /// `ordinal`. This matters because MULTI-1824's planner assembles a plan
    /// from bounded-concurrency tasks, whose completion (and thus arrival)
    /// order varies run to run; without normalizing it here, two runs over an
    /// unchanged suite would produce spuriously different bytes. A call's
    /// order within its check is left as given (replay order is meaningful);
    /// exact duplicate calls are de-duplicated first, preserving
    /// first-occurrence order (see [`dedup_calls_preserving_order`]).
    pub fn write(dir: &Path, plan: &PlanFile) -> Result<(), PlanError> {
        let mut plan = plan.clone();
        plan.requirements
            .sort_by(|a, b| (a.source.as_str(), a.ordinal).cmp(&(b.source.as_str(), b.ordinal)));
        for requirement in &mut plan.requirements {
            requirement.checks.sort_by_key(|check| check.ordinal);
            for check in &mut requirement.checks {
                check.calls = dedup_calls_preserving_order(std::mem::take(&mut check.calls));
            }
        }

        let rendered = plan.to_toml_string()?;
        let path = dir.join(PLAN_FILE_NAME);

        let mut tmp = tempfile::Builder::new()
            .prefix(".check-plan.toml.")
            .tempfile_in(dir)
            .map_err(|source| PlanError::Write {
                path: path.clone(),
                source,
            })?;
        std::io::Write::write_all(&mut tmp, rendered.as_bytes()).map_err(|source| {
            PlanError::Write {
                path: path.clone(),
                source,
            }
        })?;
        tmp.persist(&path).map_err(|persist_err| PlanError::Write {
            path: path.clone(),
            source: persist_err.error,
        })?;
        Ok(())
    }
}

/// De-duplicate exact-duplicate [`PlanCall`]s, keeping each one's first
/// occurrence and dropping later repeats, preserving the surviving calls'
/// relative order. `O(n²)` in the number of calls (a check's calls are
/// realistically dozens, not thousands) — `PlanCall` can't be hashed (it
/// embeds a `serde_json::Value`, which isn't `Eq`/`Hash`), so a `HashSet`
/// isn't an option.
fn dedup_calls_preserving_order(calls: Vec<PlanCall>) -> Vec<PlanCall> {
    let mut kept: Vec<PlanCall> = Vec::with_capacity(calls.len());
    for call in calls {
        if !kept.contains(&call) {
            kept.push(call);
        }
    }
    kept
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A failure reading, writing, or building a `.check-plan.toml`.
#[derive(Debug, Error, Diagnostic)]
pub enum PlanError {
    #[error("failed to read plan file {}: {source}", path.display())]
    #[diagnostic(code(jev::plan::io))]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse plan file {}: {source}", path.display())]
    #[diagnostic(
        code(jev::plan::parse),
        help("check that the file is valid TOML matching the `.check-plan.toml` schema")
    )]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error(
        "plan file {} has unsupported version {version} (expected {expected})",
        path.display()
    )]
    #[diagnostic(
        code(jev::plan::unknown_version),
        help("regenerate the plan with a compatible version of `multi plan`")
    )]
    UnknownVersion {
        path: PathBuf,
        version: u32,
        expected: u32,
    },

    #[error("failed to render plan as TOML: {0}")]
    #[diagnostic(code(jev::plan::serialize))]
    Serialize(#[source] toml::ser::Error),

    #[error("failed to write plan file {}: {source}", path.display())]
    #[diagnostic(code(jev::plan::write))]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("path `{path}` escapes the repository root {}", root.display())]
    #[diagnostic(
        code(jev::plan::path_escape),
        help("a check may only depend on evidence inside its repository root")
    )]
    PathEscape { path: String, root: PathBuf },

    #[error("malformed `{tool:?}` tool-call input: {reason}")]
    #[diagnostic(code(jev::plan::malformed_call))]
    MalformedInput { tool: ReadOnlyTool, reason: String },

    #[error(
        "JSON null is not representable in TOML; strip null-valued keys before storing a plan call"
    )]
    #[diagnostic(code(jev::plan::null_value))]
    NullValue,

    #[error(
        "JSON number `{0}` cannot be represented in TOML (must fit a signed 64-bit integer, or be a finite float)"
    )]
    #[diagnostic(code(jev::plan::number_out_of_range))]
    NumberOutOfRange(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    // -- fixtures -----------------------------------------------------------

    fn sample_plan() -> PlanFile {
        PlanFile::new(vec![PlanRequirement {
            title: "Authentication lives in Keystore".to_string(),
            source: "CHECKS.md".to_string(),
            ordinal: 1,
            checks: vec![PlanCheck {
                title: "Only Keystore signs JWTs".to_string(),
                ordinal: 0,
                prompt_xxh64: prompt_xxh64("Only Keystore signs JWTs", "check body"),
                decider: Decider::Agent(AgentReason::ControlFailed),
                verdict: true,
                evidence: Some("Keystore alone imports the signing key".to_string()),
                jev: Some(JevCalibration {
                    model: "jev-1.13.0".to_string(),
                    noul: 0.93,
                    control_noul: 0.04,
                    reading: Reading::Satisfied,
                }),
                calls: vec![
                    PlanCall::Read {
                        input: json!({"file_path": "src/auth/sign.rs"}),
                        xxh64: "0a1b0a1b0a1b0a1b".to_string(),
                    },
                    PlanCall::Grep {
                        input: json!({"pattern": "sign_jwt", "path": "src"}),
                        files_xxh64: "77aa77aa77aa77aa".to_string(),
                        lines_xxh64: "c0dec0dec0dec0de".to_string(),
                    },
                ],
            }],
        }])
    }

    /// A minimal, otherwise-empty check with the given `ordinal` — for tests
    /// that only care about requirement/check *ordering*, not content.
    fn minimal_check(ordinal: u32, title: &str) -> PlanCheck {
        PlanCheck {
            title: title.to_string(),
            ordinal,
            prompt_xxh64: prompt_xxh64(title, ""),
            decider: Decider::Jev,
            verdict: true,
            evidence: None,
            jev: None,
            calls: vec![],
        }
    }

    // -- schema / rendering ---------------------------------------------------

    #[test]
    fn renders_the_shape_from_the_ticket_example() {
        let plan = sample_plan();
        let rendered = plan.to_toml_string().unwrap();

        assert!(rendered.contains("version = 1"));
        assert!(rendered.contains("[[requirement]]"));
        assert!(rendered.contains(r#"title = "Authentication lives in Keystore""#));
        assert!(rendered.contains(r#"source = "CHECKS.md""#));
        assert!(rendered.contains("ordinal = 1"));

        assert!(rendered.contains("[[requirement.check]]"));
        assert!(rendered.contains(r#"title = "Only Keystore signs JWTs""#));
        assert!(rendered.contains(r#"decider = "agent""#));
        assert!(rendered.contains("verdict = true"));

        // `jev` must be an inline table on ONE line, not a `[section]`.
        assert!(rendered.contains(
            r#"jev = { model = "jev-1.13.0", noul = 0.93, control_noul = 0.04, reading = "satisfied" }"#
        ));
        assert!(!rendered.contains("[requirement.check.jev]"));

        assert!(rendered.contains(r#"agent_reason = "control failed""#));

        assert!(rendered.contains("[[requirement.check.call]]"));
        assert!(rendered.contains(r#"tool = "Read""#));
        assert!(rendered.contains(r#"input = { file_path = "src/auth/sign.rs" }"#));
        assert!(!rendered.contains("[requirement.check.call.input]"));
        assert!(rendered.contains(r#"xxh64 = "0a1b0a1b0a1b0a1b""#));

        assert!(rendered.contains(r#"tool = "Grep""#));
        assert!(rendered.contains(r#"files_xxh64 = "77aa77aa77aa77aa""#));
        assert!(rendered.contains(r#"lines_xxh64 = "c0dec0dec0dec0de""#));
    }

    #[test]
    fn truncated_call_carries_no_checksum() {
        let mut plan = sample_plan();
        plan.requirements[0].checks[0].calls = vec![PlanCall::Truncated {
            tool: ReadOnlyTool::Glob,
            input: json!({"pattern": "**/*.rs"}),
        }];
        let rendered = plan.to_toml_string().unwrap();
        // Scoped to the call section: the check-level `prompt_xxh64` field
        // (rendered earlier in the same document) also contains the
        // substring "xxh64", so a whole-document check would false-positive.
        let call_section = rendered
            .split("[[requirement.check.call]]")
            .nth(1)
            .expect("a call section was rendered");

        assert!(call_section.contains(r#"tool = "Glob""#));
        assert!(call_section.contains("truncated = true"));
        assert!(!call_section.contains("xxh64"));
    }

    // -- round trip -----------------------------------------------------------

    #[test]
    fn round_trips_write_then_load() {
        let dir = TempDir::new().unwrap();
        let plan = sample_plan();

        PlanStore::write(dir.path(), &plan).unwrap();
        let loaded = PlanStore::load(dir.path()).unwrap().expect("plan exists");

        assert_eq!(loaded, plan);
    }

    /// A true write → load round trip is exactly where a hand-written
    /// renderer breaks: every scalar/key here must be emitted through the
    /// `toml` crate's own escaping (never a manually `format!`-quoted
    /// string), or this test fails to reparse to the same value.
    #[test]
    fn round_trips_adversarial_strings_and_shapes() {
        let dir = TempDir::new().unwrap();

        let requirement_title = "Weird req title with ]] and # and = and \"double\" 'single'";
        let check_title = "Matches fn\\s+sign_\\w+\\(.*\"\\) and a windows path C:\\path\\to\\file";
        let evidence = "line one\n\tindented line two\r\nline three, trailing space \nlast line — emoji: 🎉🚀 — control: \u{1}\u{7}\u{1b} end";

        let input = json!({
            "file_path": "a.rs",
            "nested": {
                "obj": {"c": "d", "n": -7},
                "arr": [1, -2, 3.5, true, "s"],
            },
            "empty_string": "",
            "padded": "  leading and trailing space  ",
            "quotes": "she said \"hi\" and it's a \\ backslash",
            "regex": "fn\\s+sign_\\w+\\(.*\"\\)",
            "windows_path": "C:\\path\\to\\file",
            "control_chars": "tab\tnewline\nreturn\rbell\u{7}esc\u{1b}",
            "emoji": "🎉🚀🧵",
            "key with space": 1,
            "key.with.dot": 2,
            "negative": -42,
            "float": -3.5,
        });

        let plan = PlanFile::new(vec![
            PlanRequirement {
                title: requirement_title.to_string(),
                source: "CHECKS.md".to_string(),
                ordinal: 0,
                checks: vec![
                    PlanCheck {
                        title: check_title.to_string(),
                        ordinal: 0,
                        prompt_xxh64: prompt_xxh64(check_title, evidence),
                        decider: Decider::Agent(AgentReason::TruncatedDiscovery),
                        verdict: false,
                        evidence: Some(evidence.to_string()),
                        jev: None,
                        calls: vec![
                            PlanCall::Read {
                                input: input.clone(),
                                xxh64: "0000000000000000".to_string(),
                            },
                            // Empty `input` (`{}`).
                            PlanCall::Glob {
                                input: json!({}),
                                xxh64: "1111111111111111".to_string(),
                            },
                        ],
                    },
                    // A check with zero calls.
                    minimal_check(1, "no calls"),
                ],
            },
            // A requirement with zero checks.
            PlanRequirement {
                title: "empty requirement".to_string(),
                source: "CHECKS.md".to_string(),
                ordinal: 1,
                checks: vec![],
            },
        ]);

        PlanStore::write(dir.path(), &plan).unwrap();
        let loaded = PlanStore::load(dir.path()).unwrap().expect("plan exists");
        assert_eq!(loaded, plan);
    }

    #[test]
    fn missing_plan_file_loads_as_none() {
        let dir = TempDir::new().unwrap();
        assert!(PlanStore::load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn write_is_atomic_and_leaves_no_stray_tempfiles() {
        let dir = TempDir::new().unwrap();
        PlanStore::write(dir.path(), &sample_plan()).unwrap();

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec![PLAN_FILE_NAME]);
    }

    #[test]
    fn output_is_byte_identical_across_repeated_writes() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        let plan = sample_plan();

        PlanStore::write(dir_a.path(), &plan).unwrap();
        PlanStore::write(dir_b.path(), &plan).unwrap();

        let a = std::fs::read_to_string(dir_a.path().join(PLAN_FILE_NAME)).unwrap();
        let b = std::fs::read_to_string(dir_b.path().join(PLAN_FILE_NAME)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn write_output_is_independent_of_requirement_and_check_assembly_order() {
        // Two requirements (by `(source, ordinal)`), each with two checks
        // (by `ordinal`), assembled in two different orders — arrival order
        // varies run to run once MULTI-1824's planner collects results from
        // bounded-concurrency tasks, so `write` itself must normalize it.
        let req_a = PlanRequirement {
            title: "Req A".to_string(),
            source: "CHECKS.md".to_string(),
            ordinal: 0,
            checks: vec![minimal_check(0, "A check 0"), minimal_check(1, "A check 1")],
        };
        let req_b = PlanRequirement {
            title: "Req B".to_string(),
            source: "CHECKS.md".to_string(),
            ordinal: 1,
            checks: vec![minimal_check(0, "B check 0"), minimal_check(1, "B check 1")],
        };

        let canonical = PlanFile::new(vec![req_a.clone(), req_b.clone()]);

        let mut req_a_shuffled = req_a;
        req_a_shuffled.checks.reverse();
        let mut req_b_shuffled = req_b;
        req_b_shuffled.checks.reverse();
        let shuffled = PlanFile::new(vec![req_b_shuffled, req_a_shuffled]);

        let dir_canonical = TempDir::new().unwrap();
        let dir_shuffled = TempDir::new().unwrap();
        PlanStore::write(dir_canonical.path(), &canonical).unwrap();
        PlanStore::write(dir_shuffled.path(), &shuffled).unwrap();

        let canonical_bytes =
            std::fs::read_to_string(dir_canonical.path().join(PLAN_FILE_NAME)).unwrap();
        let shuffled_bytes =
            std::fs::read_to_string(dir_shuffled.path().join(PLAN_FILE_NAME)).unwrap();
        assert_eq!(canonical_bytes, shuffled_bytes);
    }

    #[test]
    fn key_order_in_input_is_sorted_regardless_of_capture_order() {
        let mut plan = sample_plan();
        plan.requirements[0].checks[0].calls = vec![PlanCall::Grep {
            input: json!({"path": "src", "pattern": "sign_jwt", "case_insensitive": true}),
            files_xxh64: "77aa77aa77aa77aa".to_string(),
            lines_xxh64: "c0dec0dec0dec0de".to_string(),
        }];
        let rendered = plan.to_toml_string().unwrap();
        assert!(rendered.contains(
            r#"input = { case_insensitive = true, path = "src", pattern = "sign_jwt" }"#
        ));
    }

    #[test]
    fn dedups_exact_duplicate_calls_preserving_first_occurrence_order() {
        let dir = TempDir::new().unwrap();
        let mut plan = sample_plan();
        let first = PlanCall::Read {
            input: json!({"file_path": "a.rs"}),
            xxh64: "1111111111111111".to_string(),
        };
        let second = PlanCall::Read {
            input: json!({"file_path": "b.rs"}),
            xxh64: "2222222222222222".to_string(),
        };
        plan.requirements[0].checks[0].calls = vec![first.clone(), second.clone(), first.clone()];

        PlanStore::write(dir.path(), &plan).unwrap();
        let loaded = PlanStore::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.requirements[0].checks[0].calls, vec![first, second]);
    }

    // -- lookup -----------------------------------------------------------

    #[test]
    fn lookup_finds_a_matching_check() {
        let plan = sample_plan();
        let hash = prompt_xxh64("Only Keystore signs JWTs", "check body");
        let found = plan.lookup("CHECKS.md", 1, 0, &hash);
        assert_eq!(found, Some(&plan.requirements[0].checks[0]));
    }

    #[test]
    fn lookup_returns_none_on_prompt_hash_mismatch() {
        let plan = sample_plan();
        let found = plan.lookup("CHECKS.md", 1, 0, "0000000000000000");
        assert!(found.is_none());
    }

    #[test]
    fn lookup_returns_none_for_unknown_source_or_ordinals() {
        let plan = sample_plan();
        let hash = prompt_xxh64("Only Keystore signs JWTs", "check body");
        assert!(plan.lookup("CHECKS.toml", 1, 0, &hash).is_none());
        assert!(plan.lookup("CHECKS.md", 2, 0, &hash).is_none());
        assert!(plan.lookup("CHECKS.md", 1, 5, &hash).is_none());
    }

    // -- version handling -----------------------------------------------------------

    #[test]
    fn unknown_version_is_a_named_diagnostic() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(PLAN_FILE_NAME), "version = 99\n").unwrap();

        let err = PlanStore::load(dir.path()).unwrap_err();
        match &err {
            PlanError::UnknownVersion {
                path,
                version,
                expected,
            } => {
                assert_eq!(*version, 99);
                assert_eq!(*expected, CURRENT_VERSION);
                assert!(path.ends_with(PLAN_FILE_NAME));
            }
            other => panic!("expected UnknownVersion, got {other:?}"),
        }
        assert!(err.to_string().contains(PLAN_FILE_NAME));
    }

    #[test]
    fn malformed_toml_is_a_parse_diagnostic_naming_the_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(PLAN_FILE_NAME), "not valid toml =::").unwrap();

        let err = PlanStore::load(dir.path()).unwrap_err();
        assert!(matches!(err, PlanError::Parse { .. }));
        assert!(err.to_string().contains(PLAN_FILE_NAME));
    }

    /// A minimal, otherwise-valid `[[requirement.check]]` block with `body`
    /// spliced in verbatim for its `decider`/`agent_reason` lines — for
    /// testing that invalid pairing is rejected on load.
    fn minimal_plan_toml_with_check_body(body: &str) -> String {
        format!(
            "version = 1\n\n\
             [[requirement]]\n\
             title = \"R\"\n\
             source = \"CHECKS.md\"\n\
             ordinal = 0\n\n\
             [[requirement.check]]\n\
             title = \"C\"\n\
             ordinal = 0\n\
             prompt_xxh64 = \"0000000000000000\"\n\
             verdict = true\n\
             {body}\n"
        )
    }

    #[test]
    fn jev_decider_with_an_agent_reason_is_rejected_on_load() {
        let dir = TempDir::new().unwrap();
        let toml = minimal_plan_toml_with_check_body(
            "decider = \"jev\"\nagent_reason = \"control failed\"",
        );
        std::fs::write(dir.path().join(PLAN_FILE_NAME), toml).unwrap();

        let err = PlanStore::load(dir.path()).unwrap_err();
        assert!(matches!(err, PlanError::Parse { .. }));
        assert!(err.to_string().contains(PLAN_FILE_NAME));
    }

    #[test]
    fn agent_decider_without_an_agent_reason_is_rejected_on_load() {
        let dir = TempDir::new().unwrap();
        let toml = minimal_plan_toml_with_check_body("decider = \"agent\"");
        std::fs::write(dir.path().join(PLAN_FILE_NAME), toml).unwrap();

        let err = PlanStore::load(dir.path()).unwrap_err();
        assert!(matches!(err, PlanError::Parse { .. }));
        assert!(err.to_string().contains(PLAN_FILE_NAME));
    }

    #[test]
    fn jev_decider_without_an_agent_reason_loads_fine() {
        // Sanity check that `minimal_plan_toml_with_check_body` itself
        // produces a valid plan when the pairing IS correct, so the two
        // rejection tests above are testing the pairing rule and not some
        // unrelated fixture mistake.
        let dir = TempDir::new().unwrap();
        let toml = minimal_plan_toml_with_check_body("decider = \"jev\"");
        std::fs::write(dir.path().join(PLAN_FILE_NAME), toml).unwrap();

        let loaded = PlanStore::load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.requirements[0].checks[0].decider, Decider::Jev);
    }

    // -- relativize -----------------------------------------------------------

    #[test]
    fn a_requirements_file_in_a_subdirectory_stores_repository_root_relative_paths() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"file_path": "/sandbox/root/services/keystore/src/sign.rs"});
        let relativized = relativize(ReadOnlyTool::Read, &input, sandbox_root).unwrap();
        assert_eq!(
            relativized,
            json!({"file_path": "services/keystore/src/sign.rs"})
        );
    }

    #[test]
    fn omitted_grep_path_is_stored_as_dot() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"pattern": "sign_jwt"});
        let relativized = relativize(ReadOnlyTool::Grep, &input, sandbox_root).unwrap();
        assert_eq!(relativized, json!({"pattern": "sign_jwt", "path": "."}));
    }

    #[test]
    fn omitted_glob_path_is_stored_as_dot() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"pattern": "**/*.rs"});
        let relativized = relativize(ReadOnlyTool::Glob, &input, sandbox_root).unwrap();
        assert_eq!(relativized, json!({"pattern": "**/*.rs", "path": "."}));
    }

    #[test]
    fn null_valued_keys_are_stripped() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"file_path": "/sandbox/root/a.rs", "limit": null});
        let relativized = relativize(ReadOnlyTool::Read, &input, sandbox_root).unwrap();
        assert_eq!(relativized, json!({"file_path": "a.rs"}));
    }

    #[test]
    fn path_escaping_the_root_via_absolute_path_is_rejected() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"file_path": "/etc/passwd"});
        let err = relativize(ReadOnlyTool::Read, &input, sandbox_root).unwrap_err();
        assert!(matches!(err, PlanError::PathEscape { .. }));
    }

    #[test]
    fn path_escaping_the_root_via_dot_dot_is_rejected() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"file_path": "/sandbox/root/../../etc/passwd"});
        let err = relativize(ReadOnlyTool::Read, &input, sandbox_root).unwrap_err();
        assert!(matches!(err, PlanError::PathEscape { .. }));
    }

    #[test]
    fn dot_dot_that_stays_inside_the_root_is_accepted() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"file_path": "/sandbox/root/services/../services/keystore/src/sign.rs"});
        let relativized = relativize(ReadOnlyTool::Read, &input, sandbox_root).unwrap();
        assert_eq!(
            relativized,
            json!({"file_path": "services/keystore/src/sign.rs"})
        );
    }

    #[test]
    fn root_itself_relativizes_to_dot() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"path": "/sandbox/root"});
        let relativized = relativize(ReadOnlyTool::Grep, &input, sandbox_root).unwrap();
        assert_eq!(relativized, json!({"path": "."}));
    }

    #[test]
    fn macos_private_var_and_var_spellings_of_the_root_are_equivalent() {
        // A sandbox root spelled the non-canonical way (`/var/...`, what
        // `$TMPDIR` commonly yields on macOS)...
        let sandbox_root = Path::new("/var/folders/xy/sandbox");
        // ...and a captured path spelled the canonicalized way
        // (`/private/var/...`) must still relativize onto the same root.
        let input = json!({"file_path": "/private/var/folders/xy/sandbox/src/lib.rs"});
        let relativized = relativize(ReadOnlyTool::Read, &input, sandbox_root).unwrap();
        assert_eq!(relativized, json!({"file_path": "src/lib.rs"}));
    }

    #[test]
    fn macos_private_var_root_and_var_spelled_path_are_equivalent() {
        let sandbox_root = Path::new("/private/var/folders/xy/sandbox");
        let input = json!({"file_path": "/var/folders/xy/sandbox/src/lib.rs"});
        let relativized = relativize(ReadOnlyTool::Read, &input, sandbox_root).unwrap();
        assert_eq!(relativized, json!({"file_path": "src/lib.rs"}));
    }

    #[test]
    fn absolute_glob_pattern_under_the_root_is_relativized() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"pattern": "/sandbox/root/src/**/*.rs"});
        let relativized = relativize(ReadOnlyTool::Glob, &input, sandbox_root).unwrap();
        assert_eq!(relativized, json!({"pattern": "src/**/*.rs", "path": "."}));
    }

    #[test]
    fn absolute_glob_pattern_escaping_the_root_is_rejected() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"pattern": "/etc/**/*.conf"});
        let err = relativize(ReadOnlyTool::Glob, &input, sandbox_root).unwrap_err();
        assert!(matches!(err, PlanError::PathEscape { .. }));
    }

    #[test]
    fn relative_glob_pattern_is_left_untouched() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"pattern": "**/*.rs"});
        let relativized = relativize(ReadOnlyTool::Glob, &input, sandbox_root).unwrap();
        assert_eq!(relativized["pattern"], json!("**/*.rs"));
    }

    #[test]
    fn non_string_path_key_is_malformed_input() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!({"file_path": 5});
        let err = relativize(ReadOnlyTool::Read, &input, sandbox_root).unwrap_err();
        assert!(matches!(err, PlanError::MalformedInput { .. }));
    }

    #[test]
    fn non_object_input_is_malformed_input() {
        let sandbox_root = Path::new("/sandbox/root");
        let input = json!("not an object");
        let err = relativize(ReadOnlyTool::Read, &input, sandbox_root).unwrap_err();
        assert!(matches!(err, PlanError::MalformedInput { .. }));
    }

    // -- integer/float determinism -----------------------------------------------------------

    #[test]
    fn integer_and_float_inputs_round_trip_distinctly() {
        let dir = TempDir::new().unwrap();
        let mut plan = sample_plan();
        plan.requirements[0].checks[0].calls = vec![PlanCall::Grep {
            input: json!({"path": "src", "pattern": "x", "head_limit": 5, "ratio": 5.0}),
            files_xxh64: "77aa77aa77aa77aa".to_string(),
            lines_xxh64: "c0dec0dec0dec0de".to_string(),
        }];

        PlanStore::write(dir.path(), &plan).unwrap();
        let loaded = PlanStore::load(dir.path()).unwrap().unwrap();
        let PlanCall::Grep { input, .. } = &loaded.requirements[0].checks[0].calls[0] else {
            panic!("expected a Grep call");
        };
        assert!(input["head_limit"].is_i64());
        assert_eq!(input["head_limit"], json!(5));
        assert!(input["ratio"].is_f64());
        assert_eq!(input["ratio"], json!(5.0));
    }

    // -- path joining -----------------------------------------------------------

    #[test]
    fn join_forward_slash_joins_multiple_components_with_forward_slashes() {
        assert_eq!(
            join_forward_slash(Path::new("services/keystore/src/sign.rs")),
            "services/keystore/src/sign.rs"
        );
    }

    #[test]
    fn join_forward_slash_of_a_single_component_has_no_slash() {
        assert_eq!(join_forward_slash(Path::new("a.rs")), "a.rs");
    }

    // -- checksum helpers -----------------------------------------------------------

    #[test]
    fn xxh64_hex_is_fixed_width_and_stable() {
        let a = xxh64_hex(b"hello world");
        assert_eq!(a.len(), 16);
        assert_eq!(a, xxh64_hex(b"hello world"));
        assert_ne!(a, xxh64_hex(b"hello world!"));
    }

    #[test]
    fn prompt_xxh64_is_stable_and_distinguishes_the_title_prompt_split() {
        let a = prompt_xxh64("a", "bc");
        let b = prompt_xxh64("ab", "c");
        assert_ne!(a, b, "differing title/prompt splits must not collide");
        assert_eq!(a, prompt_xxh64("a", "bc"));
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn prompt_xxh64_changes_when_prompt_changes() {
        assert_ne!(
            prompt_xxh64("title", "prompt one"),
            prompt_xxh64("title", "prompt two"),
        );
    }
}
