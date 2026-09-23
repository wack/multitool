//! Building the Jev verification question from replayed evidence, and
//! deciding a check from Jev's answer (MULTI-1823).
//!
//! One `POST /v1/systemone` request carries **two** questions over the same
//! `state`: an affirmative Noul (`satisfied`) that gates the decision, and a
//! record-only Choice (`reading`) that never does — see [`JevDecision`] and
//! the "Choice never gates" section below. Batching them costs nothing extra:
//! TypeSafe's own cookbook on asking several questions per call reports that
//! "each question is scored on its own against the document, so its answer
//! doesn't depend on what else is in the request" and that batching many
//! questions over one document is over an order of magnitude cheaper than one
//! call per question, "since the document dominates every request"
//! (<https://docs.typesafe.ai/cookbooks/parallel_questions.md>).
//!
//! ## State shape
//!
//! `{ requirement: { title, declared_in }, check: { title, prompt },
//! evidence: [{ tool, input, output }] }` — a named-field object, per
//! <https://docs.typesafe.ai/concepts/state.md>'s guidance to "use objects
//! for most requests" so "each part of the state has a descriptive name and
//! its relationships remain clear," and to "separate content from
//! questions": state carries every fact, the two questions carry only the
//! judgments.
//!
//! `requirement.declared_in` is the requirements file's repository-root-
//! relative path (MULTI-1834's scoping fact — a requirement declared in
//! `services/keystore/CHECKS.toml` is implicitly about that service, the same
//! way evidence paths are root-relative). This stack doesn't implement
//! MULTI-1834 yet, so [`Requirement::declared_in`] is a plain `&str` a caller
//! supplies directly, not something resolved from a sandboxed repository root.
//!
//! `evidence` is a caller-supplied parameter, not something this module
//! fetches — that's what lets a caller issue the negative control (the
//! byte-for-byte-identical request with `evidence: []`, used at plan time to
//! confirm Jev doesn't rubber-stamp a check it has no evidence for). This
//! module is deliberately thin on `super::replay`: an [`Evidence`] entry
//! carries only a replayed call's `tool`, `input`, root-normalized `output`,
//! and `windowed` flag — never `super::replay::ReplayedCall`'s checksum/
//! freshness bookkeeping, which has nothing to do with building a request
//! and would otherwise couple this module to replay's shape (replay's own
//! plan-review branch may still change it). `windowed` is never sent to Jev
//! (`#[serde(skip_serializing)]`) — it exists purely to drive
//! [`merge_overlapping_reads`], below.
//!
//! `input.file_path` (for `Read` evidence) is expected to already be
//! repository-root-relative, exactly as [`super::plan_file::relativize`]
//! (MULTI-1820) produces it — this module doesn't re-derive that from a
//! sandbox root.
//!
//! ### Merging overlapping `Read`s
//!
//! Two evidence entries for the same file are merged, conservatively:
//!
//! * Duplicate whole-file `Read`s of the same `file_path` (an agent reading
//!   the same file twice at plan time) collapse to the first occurrence.
//! * A windowed `Read` (`Evidence::windowed`) of a file that *also* has a
//!   whole-file `Read` for the same `file_path` is dropped outright — its
//!   content is a strict subset of what the whole-file read already sends,
//!   so keeping both would send the same lines twice.
//! * Two windowed `Read`s of the same file, with no whole-file `Read` of it,
//!   are **never spliced** into one, even when their ranges overlap or are
//!   adjacent — doing that correctly requires parsing the line-numbered
//!   output and re-deriving the union range exactly, which this module does
//!   not attempt (see [`merge_overlapping_reads`]). Both entries are kept.
//! * Everything else (`Grep`/`Glob` entries, and `Read`s of different files)
//!   is left untouched.
//!
//! Whether a `Read` is "whole-file" is read directly from
//! [`Evidence::windowed`] — **not** re-derived from whether `input` carries
//! `offset`/`limit` — because a `Read` can fail to see the whole file even
//! when it omitted both: `super::replay::ReplayedCall::windowed` is also set
//! when an unwindowed call's output happened to hit the read tool's default
//! line cap exactly (see that field's docs). Treating such a capped read as
//! "whole" would wrongly drop a later, genuinely windowed read of the file's
//! remaining lines — see the
//! `a_capped_read_marked_windowed_does_not_subsume_a_later_window_of_the_same_file`
//! test. The caller (the future `multi plan`/`multi check` wiring) is
//! expected to fill `Evidence::windowed` straight from
//! `ReplayedCall::windowed`.
//!
//! Two evidence entries are considered the same file when their (already
//! root-relative) `file_path`s are equal after stripping a redundant leading
//! `./` — e.g. `./a.rs` and `a.rs` — see [`normalized_file_path`]. This is a
//! defensive normalization of trivially-equivalent spellings, not a general
//! path canonicalization.
//!
//! Only `Read` is handled: `Grep`/`Glob` results are already
//! deduplicated exactly by [`super::plan_file::dedup_calls_preserving_order`]
//! at the point a plan is written, so a byte-identical repeat isn't something
//! replay can hand this module in the first place; a *merge* (as opposed to
//! exact-duplicate removal) only ever makes sense for `Read`, since only
//! `Read` can be windowed to a subset of another call's content.
//!
//! Evidence order is otherwise preserved exactly as given — see
//! [`merge_overlapping_reads`] — so serialization stays deterministic for a
//! given evidence list.
//!
//! ## The two questions
//!
//! Per <https://docs.typesafe.ai/primitives/noul.md>'s wording guidance —
//! "phrase for affirmative meaning: structure questions so high values
//! indicate yes" and "avoid compound conditions: ask one proposition per
//! Noul" — and <https://docs.typesafe.ai/model-jaggedness/jev-1.13.md>'s
//! warning that "Jev answers the question you wrote, not the one you meant"
//! and that "instructions carrying double negatives or complex indirection
//! are answered less reliably," both questions are phrased as single,
//! affirmative propositions that reference `state` by backticked path
//! (<https://docs.typesafe.ai/concepts/state.md>) rather than restating it.
//! Question ids (`"satisfied"`, `"reading"`) are code-only keys into
//! `answers`; the visible judgment lives entirely in `instructions`/
//! `criteria`.
//!
//! * **`satisfied`** (Noul, <https://docs.typesafe.ai/primitives/noul.md>):
//!   "Does `evidence` affirmatively demonstrate that the requirement
//!   described by `check.prompt` is satisfied?" `criteria.true`/`false` match
//!   the ticket's wording exactly — see [`noul_question`].
//! * **`reading`** (Choice, <https://docs.typesafe.ai/primitives/choice.md>):
//!   "Which reading best characterizes what `evidence` shows about the
//!   requirement described by `check.prompt`: satisfied, violated, or
//!   insufficient?", with one `criteria` entry per option — see
//!   [`choice_question`]. Its answer is surfaced (as [`ChoiceOutcome`]) but
//!   never consulted to compute the decision.
//!
//! ## Choice never gates
//!
//! [`JevDecision`]'s variant is a function of the Noul answer and
//! [`crate::checks::config::JevConfig::threshold`] **alone**. [`verify`]
//! makes this structurally obvious: it computes `satisfied` from the Noul
//! answer before it even looks at the Choice answer, so the Choice literally
//! cannot influence which arm gets built. The
//! `choice_never_gates_the_decision_even_when_contradicting` test below
//! scripts a mock response where the Choice flatly contradicts the Noul (a
//! high Noul paired with `reading: "violated"`, and vice versa) and asserts
//! the decision follows the Noul regardless.
//!
//! A response can carry a Noul answer with no Choice answer (or an
//! unparseable, wrong-typed, or out-of-range one) without that invalidating
//! an otherwise-valid decision: [`ChoiceOutcome`] is carried as `Option`,
//! `None` whenever the Choice answer is missing, isn't shaped like a Choice,
//! doesn't parse into [`super::plan_file::Reading`], or carries a
//! `confidence`/probability outside `0.0..=1.0` — see [`extract_choice`].
//! [`SystemOneResponse::answers`] itself is a map of raw, unparsed JSON
//! values for exactly this reason (see `types.rs`): eagerly parsing every
//! entry into a typed [`Answer`] would let a malformed *record-only* Choice
//! answer fail the *whole response*, silently promoting it to something that
//! gates the decision after all.
//!
//! Only a missing/malformed/out-of-range **Noul** answer fails the request
//! (as [`JevError::Transport`], the same variant [`super::client::JevClient`]
//! already uses for a response that doesn't match the documented shape) —
//! see [`extract_noul`]. `noul` is validated to be finite and within
//! `0.0..=1.0` before it's trusted for a decision: an API bug returning e.g.
//! `1.2` must not silently become `Satisfied`, and a `NaN` must not silently
//! compare `false` against `threshold`.
//!
//! ## Budget
//!
//! <https://docs.typesafe.ai/models.md> documents "32k tokens for `state`
//! plus the longest question" as Jev's per-request budget. This module
//! estimates tokens over `state` **and** both questions combined (a
//! deliberately more conservative bound than "the longest question alone")
//! from byte length divided by 3 and rounded up — "source code tokenizes
//! densely — assume ~3 bytes/token, not 4," per the ticket — and, when the
//! estimate exceeds [`TOKEN_BUDGET`], returns [`JevDecision::OverBudget`]
//! **without calling the API at all** (see [`verify`] and the
//! `over_budget_evidence_makes_zero_http_calls` test).
//!
//! TypeSafe's own OpenAPI spec models every `422` as the generic FastAPI
//! `HTTPValidationError` shape (`{"detail": [{"loc": [...], "msg": "...",
//! "type": "..."}]}`), with no dedicated error code distinguishing "the
//! request is too large for the model's context" from any other validation
//! failure — confirmed by fetching the live spec directly; none of the
//! Python/JavaScript SDK exception docs document one either. In the absence
//! of a documented shape, [`looks_like_context_limit`] prefers the
//! structured `detail[].msg` when the body parses as that shape (falling
//! back to the raw body text otherwise) and matches on wording: the text
//! must mention "context" together with one of "token"/"length"/"exceed"/
//! "too long"/"too many", or contain "maximum context" outright — see
//! [`mentions_context_limit`].
//!
//! This is deliberately narrower than "token" + ("context" OR "limit"): a
//! genuinely unrelated `422` can easily mention "token" (e.g. an invalid API
//! token) or "limit" (e.g. Choice's own 255-option limit) without being
//! about the context window, and MULTI-1825 must **abort** on a genuine
//! `Invalid`, not silently escalate it to an agent as if it were merely
//! over budget. `OverBudget` only ever *downgrades* a hard `Err`, so a false
//! positive here is a correctness bug, not just noise — a match is logged at
//! `warn` with the full body (see [`verify`]) precisely so a misclassification
//! is visible rather than silently masked. See the `_maps_to_over_budget`/
//! `_stays_an_err`/`_near_miss_...` tests.
//!
//! The estimate is logged alongside the response's `usage.input_tokens` at
//! `debug` (see [`verify`]) so the byte-per-token ratio can be tuned later.
//!
//! ## Resolved model id
//!
//! A non-`OverBudget` [`JevDecision`] carries `response.model` — the
//! concrete model that actually answered (e.g. `"jev-1.13.0"`) — not
//! [`crate::checks::config::JevConfig::model`], which may be a floating alias
//! like `"jev-latest"`. <https://docs.typesafe.ai/models.md> warns that a
//! threshold calibrated against one version can silently drift if the alias
//! it was calibrated under starts resolving to a new release; recording the
//! resolved id is what lets a caller (MULTI-1826's self-healing, MULTI-1832's
//! shadow report) notice that drift instead of assuming the model never
//! changed.

use std::collections::HashSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::checks::config::JevConfig;
use crate::checks::executor::ReadOnlyTool;
use crate::checks::model::Check;

use super::client::JevClient;
use super::error::JevError;
use super::plan_file::{Reading, ReadingProbabilities};
use super::types::{
    Answer, ChoiceQuestion, NoulCriteria, NoulQuestion, Question, SystemOneRequest,
    SystemOneResponse,
};

/// The `answers` key for the gating Noul question.
const NOUL_QUESTION_ID: &str = "satisfied";
/// The `answers` key for the record-only Choice question.
const CHOICE_QUESTION_ID: &str = "reading";

/// Jev's documented per-request budget (<https://docs.typesafe.ai/models.md>:
/// "32k tokens for `state` plus the longest question") — see the module docs
/// for how this module estimates against it.
const TOKEN_BUDGET: u64 = 32_000;
/// Conservative bytes-per-token divisor for the estimate (see the module
/// docs and the ticket: "assume ~3 bytes/token, not 4").
const BYTES_PER_TOKEN: u64 = 3;

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

/// The requirement a check belongs to, as sent in `state.requirement`.
/// Deliberately not `crate::checks::model::Requirement` — that type carries
/// `filepath: PathBuf` and an aggregate `checks: Vec<Check>`, neither of
/// which fits a single request's state (see the module docs on
/// `declared_in`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement<'a> {
    pub title: &'a str,
    /// The requirements file's repository-root-relative path (e.g.
    /// `"services/keystore/CHECKS.toml"`) — see the module docs.
    pub declared_in: &'a str,
}

/// One piece of evidence: a single replayed read-only tool call's identity
/// and root-normalized output, sent in `state.evidence` as `{tool, input,
/// output}`. See the module docs for why this is its own type rather than
/// `super::replay::ReplayedCall`.
///
/// `input.file_path` (for `Read` evidence) must already be root-relative,
/// exactly as [`super::plan_file::relativize`] produces it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evidence {
    pub tool: ReadOnlyTool,
    pub input: Value,
    pub output: String,
    /// Whether the replayed `Read` this evidence came from saw only part of
    /// its file — mirrors `super::replay::ReplayedCall::windowed` (the
    /// caller fills this straight from that field). Drives
    /// [`merge_overlapping_reads`] only; never sent to Jev
    /// (`#[serde(skip_serializing)]`, so the wire shape stays exactly
    /// `{tool, input, output}`). Meaningless — and always `false` — for
    /// `Grep`/`Glob`.
    #[serde(skip_serializing)]
    pub windowed: bool,
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// The record-only Choice answer paired with a non-`OverBudget`
/// [`JevDecision`]: which of [`Reading`]'s three options Jev selected, and
/// its derived confidence (<https://docs.typesafe.ai/confidence.md>).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChoiceOutcome {
    pub reading: Reading,
    /// `0.0..=1.0`.
    pub confidence: f64,
    /// Every option's probability, or `None` if the answer omitted any of
    /// the three — a partial distribution isn't recorded as if it were whole.
    pub probabilities: Option<ReadingProbabilities>,
}

/// The outcome of asking Jev to verify one check against its evidence. See
/// the module docs' "Choice never gates" section for why `choice` never
/// influences which variant this is.
#[derive(Debug, Clone, PartialEq)]
pub enum JevDecision {
    /// `noul >= threshold` (the boundary is inclusive: `noul == threshold`
    /// is `Satisfied`).
    Satisfied {
        noul: f64,
        /// The concrete model that answered — see the module docs.
        model: String,
        choice: Option<ChoiceOutcome>,
    },
    /// `noul < threshold`.
    NotVerified {
        noul: f64,
        model: String,
        choice: Option<ChoiceOutcome>,
    },
    /// The estimated (or, failing that, API-rejected) request exceeded Jev's
    /// context budget — see the module docs. No HTTP call was necessarily
    /// made in the estimate-only case.
    OverBudget,
}

// ---------------------------------------------------------------------------
// Agreement (calibration only — `multi check` uses only `threshold`)
// ---------------------------------------------------------------------------

/// What a calibration run expected a check's verdict to be — including the
/// negative control, which is always a [`Expected::Fail`] (see
/// [`agreement`]'s docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expected {
    Pass,
    /// Also covers the empty-evidence negative control: it must be rejected
    /// exactly like a genuine failure would be.
    Fail,
}

/// Whether a Jev `noul` agreed with a calibration run's expectation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agreement {
    Agrees,
    Disagrees,
    Uncertain,
}

/// Compare a calibration run's `expected` verdict against a Jev `noul`,
/// using `low = 1 - threshold` as the symmetric floor below `threshold`'s
/// ceiling: `noul >= threshold` agrees with [`Expected::Pass`] (and
/// disagrees with [`Expected::Fail`]); `noul <= low` agrees with
/// [`Expected::Fail`] (and disagrees with [`Expected::Pass`]); anything else
/// is [`Agreement::Uncertain`].
///
/// `threshold` is validated to `(0, 1]` by
/// [`crate::checks::config::jev::resolve_jev`], but nothing stops a value
/// `<= 0.5`, at which point `low >= threshold` and the two bands **cross**:
/// a single `noul` can satisfy both the "agrees" condition and the
/// "disagrees" condition for the same `expected` side at once (e.g.
/// `threshold = 0.3`, `low = 0.7`, `noul = 0.5` satisfies both `noul >=
/// threshold` and `noul <= low`). Rather than pick a side arbitrarily on
/// that contradiction, this function treats it as `Uncertain` too —
/// calibration's whole job is deciding whether Jev can be trusted, and a
/// formula that contradicts itself must not silently resolve to "trust it"
/// or "distrust it." See the `agreement_bands_crossing_*` tests.
pub fn agreement(expected: Expected, noul: f64, threshold: f64) -> Agreement {
    let low = 1.0 - threshold;
    let (agrees, disagrees) = match expected {
        Expected::Pass => (noul >= threshold, noul <= low),
        Expected::Fail => (noul <= low, noul >= threshold),
    };
    match (agrees, disagrees) {
        (true, false) => Agreement::Agrees,
        (false, true) => Agreement::Disagrees,
        _ => Agreement::Uncertain,
    }
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// Ask Jev to verify `check` (belonging to `requirement`) against `evidence`,
/// and decide the [`JevDecision`]. See the module docs for the full request
/// shape, budget handling, and why the Choice answer never gates the
/// decision.
pub async fn verify(
    client: &JevClient,
    config: &JevConfig,
    requirement: &Requirement<'_>,
    check: &Check,
    evidence: &[Evidence],
) -> Result<JevDecision, JevError> {
    let request = build_request(&config.model, requirement, check, evidence);
    let estimate = estimate_tokens(&request.state, &request.questions);

    if estimate > TOKEN_BUDGET {
        tracing::debug!(
            estimate_tokens = estimate,
            budget = TOKEN_BUDGET,
            "jev request estimated over budget; skipping the API call",
        );
        return Ok(JevDecision::OverBudget);
    }

    let response = match client.ask(&request).await {
        Ok(response) => response,
        Err(JevError::Invalid { body }) if looks_like_context_limit(&body) => {
            // `warn`, not `debug`: this heuristic (see the module docs)
            // downgrades a hard `Err` to `OverBudget`, so a misclassified
            // genuine validation bug would otherwise be silently masked as
            // "escalate to an agent later" instead of surfacing now. The
            // full body is logged so a misclassification is diagnosable.
            tracing::warn!(
                estimate_tokens = estimate,
                body = %body,
                "jev 422 matched the context-limit heuristic; treating as OverBudget",
            );
            return Ok(JevDecision::OverBudget);
        }
        Err(err) => return Err(err),
    };

    tracing::debug!(
        estimate_tokens = estimate,
        input_tokens = response.usage.input_tokens,
        "jev token estimate vs. actual usage",
    );

    let noul = extract_noul(&response, NOUL_QUESTION_ID)?;

    // `JevDecision`'s variant is a function of `noul` and `config.threshold`
    // ALONE, computed here before the Choice answer is even looked at — see
    // the module docs' "Choice never gates" section.
    let satisfied = noul >= config.threshold;

    let choice = extract_choice(&response, CHOICE_QUESTION_ID);
    let model = response.model;

    Ok(if satisfied {
        JevDecision::Satisfied {
            noul,
            model,
            choice,
        }
    } else {
        JevDecision::NotVerified {
            noul,
            model,
            choice,
        }
    })
}

/// Build the request [`verify`] sends, without sending it — split out so
/// snapshot tests can inspect the exact wire shape without a mock server.
pub fn build_request(
    model: &str,
    requirement: &Requirement<'_>,
    check: &Check,
    evidence: &[Evidence],
) -> SystemOneRequest {
    let merged = merge_overlapping_reads(evidence);
    let state = build_state(requirement, check, &merged);

    // Insertion order is preserved on the wire (see the module docs): the
    // gating Noul first, then the record-only Choice.
    let mut questions = IndexMap::with_capacity(2);
    questions.insert(NOUL_QUESTION_ID.to_string(), noul_question());
    questions.insert(CHOICE_QUESTION_ID.to_string(), choice_question());

    SystemOneRequest {
        state,
        model: model.to_string(),
        questions,
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct RequirementState<'a> {
    title: &'a str,
    declared_in: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct CheckState<'a> {
    title: &'a str,
    prompt: &'a str,
}

#[derive(Debug, Clone, Serialize)]
struct VerificationState<'a> {
    requirement: RequirementState<'a>,
    check: CheckState<'a>,
    evidence: &'a [Evidence],
}

fn build_state(requirement: &Requirement<'_>, check: &Check, evidence: &[Evidence]) -> Value {
    let state = VerificationState {
        requirement: RequirementState {
            title: requirement.title,
            declared_in: requirement.declared_in,
        },
        check: CheckState {
            title: &check.title,
            prompt: check.prompt(),
        },
        evidence,
    };
    // Infallible: every field is a string, a `ReadOnlyTool` (a plain
    // fieldless enum), a `serde_json::Value`, or a slice thereof — none of
    // which `serde_json::to_value` can fail to serialize.
    serde_json::to_value(&state).expect("VerificationState always serializes")
}

// ---------------------------------------------------------------------------
// Merging overlapping Reads — see the module docs.
// ---------------------------------------------------------------------------

/// Merge overlapping `Read` evidence of the same file, conservatively — see
/// the module docs for the exact rule. Preserves the relative order of
/// surviving entries (first-occurrence order), so serialization stays
/// deterministic for a given `evidence` slice.
fn merge_overlapping_reads(evidence: &[Evidence]) -> Vec<Evidence> {
    let mut whole_file_paths: HashSet<&str> = HashSet::new();
    for entry in evidence {
        if entry.tool == ReadOnlyTool::Read
            && !entry.windowed
            && let Some(path) = read_file_path(&entry.input)
        {
            whole_file_paths.insert(path);
        }
    }

    let mut seen_whole_file: HashSet<&str> = HashSet::new();
    let mut merged = Vec::with_capacity(evidence.len());
    for entry in evidence {
        if entry.tool == ReadOnlyTool::Read
            && let Some(path) = read_file_path(&entry.input)
        {
            if entry.windowed && whole_file_paths.contains(path) {
                // Fully contained in a whole-file read of the same file.
                continue;
            }
            if !entry.windowed && !seen_whole_file.insert(path) {
                // A duplicate whole-file read of the same file.
                continue;
            }
        }
        merged.push(entry.clone());
    }
    merged
}

/// A `Read` evidence entry's `file_path`, normalized for the merge
/// comparison above — see the module docs on why this is only a trivial
/// `./`-stripping normalization, not a general canonicalization.
fn read_file_path(input: &Value) -> Option<&str> {
    let raw = input.get("file_path")?.as_str()?;
    Some(normalized_file_path(raw))
}

/// Strip a redundant leading `./` (repeated, e.g. `././a.rs`) so trivially-
/// equivalent path spellings compare equal — see the module docs.
fn normalized_file_path(path: &str) -> &str {
    let mut path = path;
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    path
}

// ---------------------------------------------------------------------------
// Questions
// ---------------------------------------------------------------------------

/// [`noul_question`]'s `instructions`, named so the byte-stable snapshot
/// test can reference the exact same text rather than a hand-copied literal
/// that could silently drift from it.
const NOUL_INSTRUCTIONS: &str = "Does `evidence` affirmatively demonstrate that the requirement \
    described by `check.prompt` is satisfied?";

/// [`choice_question`]'s `instructions` — see [`NOUL_INSTRUCTIONS`].
const CHOICE_INSTRUCTIONS: &str = "Which reading best characterizes what `evidence` shows about \
    the requirement described by `check.prompt`: satisfied, violated, or insufficient?";

/// The gating Noul question — see the module docs.
fn noul_question() -> Question {
    Question::Noul(NoulQuestion {
        instructions: NOUL_INSTRUCTIONS.to_string(),
        criteria: Some(NoulCriteria {
            when_true: "The evidence affirmatively demonstrates the check is satisfied".to_string(),
            when_false: "The evidence shows a violation, or is insufficient to demonstrate \
                satisfaction"
                .to_string(),
        }),
    })
}

/// The record-only Choice question — see the module docs. Options are
/// inserted (and therefore presented to the model, and serialized) in the
/// fixed order `satisfied`, `violated`, `insufficient`.
fn choice_question() -> Question {
    let mut criteria = IndexMap::with_capacity(3);
    criteria.insert(
        "satisfied".to_string(),
        "The evidence affirmatively demonstrates the check holds".to_string(),
    );
    criteria.insert(
        "violated".to_string(),
        "The evidence shows the check does not hold".to_string(),
    );
    criteria.insert(
        "insufficient".to_string(),
        "The evidence does not address the check either way".to_string(),
    );
    Question::Choice(ChoiceQuestion {
        instructions: CHOICE_INSTRUCTIONS.to_string(),
        criteria,
    })
}

// ---------------------------------------------------------------------------
// Budget
// ---------------------------------------------------------------------------

/// Estimate the token cost of `state` plus both `questions`, conservatively
/// (bytes / [`BYTES_PER_TOKEN`], rounded up) — see the module docs.
fn estimate_tokens(state: &Value, questions: &IndexMap<String, Question>) -> u64 {
    // Infallible for the same reason as `build_state`.
    let state_bytes = serde_json::to_vec(state)
        .expect("Value always serializes")
        .len() as u64;
    let questions_bytes = serde_json::to_vec(questions)
        .expect("Question always serializes")
        .len() as u64;
    (state_bytes + questions_bytes).div_ceil(BYTES_PER_TOKEN)
}

/// Whether a `422` response body identifies the context/token limit as the
/// rejection reason — see the module docs on why this is a wording match
/// rather than a documented error code, and why it's deliberately narrow.
fn looks_like_context_limit(body: &str) -> bool {
    if let Ok(parsed) = serde_json::from_str::<ValidationErrorBody>(body) {
        return parsed.detail.iter().any(|d| mentions_context_limit(&d.msg));
    }
    mentions_context_limit(body)
}

/// Companion words that, together with "context", indicate the model's
/// context window specifically — as opposed to some other, unrelated sense
/// of "limit" (e.g. Choice's 255-option cap) or "token" (e.g. an invalid API
/// token) that a genuinely different `422` could just as easily mention. See
/// the module docs.
const CONTEXT_LIMIT_COMPANIONS: &[&str] = &["token", "length", "exceed", "too long", "too many"];

fn mentions_context_limit(text: &str) -> bool {
    let lower = text.to_lowercase();
    if lower.contains("maximum context") {
        return true;
    }
    lower.contains("context")
        && CONTEXT_LIMIT_COMPANIONS
            .iter()
            .any(|companion| lower.contains(companion))
}

/// TypeSafe's documented `422` shape: a generic FastAPI-style
/// `HTTPValidationError` (`{"detail": [{"loc": [...], "msg": "...", "type":
/// "..."}]}`) — see the module docs. Only `msg` is read; every other field
/// is irrelevant to the context-limit heuristic.
#[derive(Debug, Deserialize)]
struct ValidationErrorBody {
    detail: Vec<ValidationErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct ValidationErrorDetail {
    #[serde(default)]
    msg: String,
}

// ---------------------------------------------------------------------------
// Extracting answers
// ---------------------------------------------------------------------------

/// Whether `x` is a value Jev's documented `0.0..=1.0` probability contract
/// promises — rejects `NaN`/infinite as well as merely out-of-range values,
/// since `NaN`'s `PartialOrd` makes every direct comparison against it
/// silently `false` (see the module docs).
fn valid_probability(x: f64) -> bool {
    x.is_finite() && (0.0..=1.0).contains(&x)
}

/// Extract and validate the required, gating Noul answer for `id`.
/// [`JevError::Transport`] (not a decision) on anything that isn't exactly a
/// well-formed Noul answer: missing, wrong-shaped, or a `noul` outside
/// `0.0..=1.0` — a malformed *required* answer must never silently decide a
/// check. See the module docs.
fn extract_noul(response: &SystemOneResponse, id: &str) -> Result<f64, JevError> {
    let value = response.answers.get(id).ok_or_else(|| {
        JevError::Transport(format!("response is missing an answer for question `{id}`"))
    })?;
    let answer: Answer = serde_json::from_value(value.clone())
        .map_err(|e| JevError::Transport(format!("malformed answer for question `{id}`: {e}")))?;
    match answer {
        Answer::Noul(noul) if valid_probability(noul.noul) => Ok(noul.noul),
        Answer::Noul(noul) => Err(JevError::Transport(format!(
            "question `{id}`'s Noul answer is out of range: {} (must be finite and within 0.0..=1.0)",
            noul.noul
        ))),
        other => Err(JevError::Transport(format!(
            "expected a Noul answer for question `{id}`, got {other:?}"
        ))),
    }
}

/// Extract the record-only Choice answer for `id`, or `None` for anything
/// that isn't exactly a well-formed, in-range Choice answer that parses into
/// a known [`Reading`] — missing, malformed, wrong-shaped, an unrecognized
/// `choice` value, or a `confidence`/probability outside `0.0..=1.0`. Never
/// an error: see the module docs' "Choice never gates" section.
fn extract_choice(response: &SystemOneResponse, id: &str) -> Option<ChoiceOutcome> {
    let value = response.answers.get(id)?;
    let answer: Answer = serde_json::from_value(value.clone()).ok()?;
    let Answer::Choice(choice) = answer else {
        return None;
    };
    if !valid_probability(choice.confidence) {
        return None;
    }
    if !choice
        .probabilities
        .values()
        .copied()
        .all(valid_probability)
    {
        return None;
    }
    let reading = parse_reading(&choice.choice)?;
    let probability = |option: &str| choice.probabilities.get(option).copied();
    let probabilities = match (
        probability("satisfied"),
        probability("violated"),
        probability("insufficient"),
    ) {
        (Some(satisfied), Some(violated), Some(insufficient)) => Some(ReadingProbabilities {
            satisfied,
            violated,
            insufficient,
        }),
        _ => None,
    };
    Some(ChoiceOutcome {
        reading,
        confidence: choice.confidence,
        probabilities,
    })
}

fn parse_reading(choice: &str) -> Option<Reading> {
    match choice {
        "satisfied" => Some(Reading::Satisfied),
        "violated" => Some(Reading::Violated),
        "insufficient" => Some(Reading::Insufficient),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::error::TYPESAFE_API_KEY_VAR;
    use super::*;

    // -- fixtures -------------------------------------------------------------

    fn sample_requirement() -> Requirement<'static> {
        Requirement {
            title: "Only Keystore signs JWTs",
            declared_in: "services/keystore/CHECKS.toml",
        }
    }

    fn sample_check() -> Check {
        Check::new_prompt(
            "only-keystore-imports-key",
            "Only Keystore imports the signing key",
            "Keystore alone imports the JWT signing key; no other service does.",
        )
    }

    fn sample_evidence() -> Vec<Evidence> {
        vec![
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "services/keystore/src/sign.rs"}),
                output: "pub fn sign_jwt() { /* ... */ }\n".to_string(),
                windowed: false,
            },
            Evidence {
                tool: ReadOnlyTool::Grep,
                input: serde_json::json!({"pattern": "sign_jwt", "path": "."}),
                output: "services/keystore/src/sign.rs:1:pub fn sign_jwt() { /* ... */ }"
                    .to_string(),
                windowed: false,
            },
        ]
    }

    fn config(threshold: f64) -> JevConfig {
        JevConfig {
            model: "jev-latest".to_string(),
            threshold,
            base_url: "https://unused.invalid".to_string(),
        }
    }

    /// Mount a `200` response answering both questions, and set
    /// `TYPESAFE_API_KEY` for the call's duration. Serializes tests that
    /// mutate the env var, mirroring `client.rs`'s `with_api_key`.
    static API_KEY_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn with_api_key<F, Fut, T>(body: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let _guard = API_KEY_ENV_LOCK.lock().await;
        let previous = std::env::var(TYPESAFE_API_KEY_VAR).ok();
        // SAFETY: see `client.rs`'s `with_api_key` — this lock serializes
        // every test in this module touching the env var.
        unsafe {
            std::env::set_var(TYPESAFE_API_KEY_VAR, "test-key");
        }
        let result = body().await;
        unsafe {
            match previous {
                Some(value) => std::env::set_var(TYPESAFE_API_KEY_VAR, value),
                None => std::env::remove_var(TYPESAFE_API_KEY_VAR),
            }
        }
        result
    }

    async fn mock_answer(server: &MockServer, noul: f64, choice: &str, confidence: f64) {
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {
                    NOUL_QUESTION_ID: {"type": "noul", "noul": noul},
                    CHOICE_QUESTION_ID: {
                        "type": "choice",
                        "choice": choice,
                        "confidence": confidence,
                        "probabilities": {"satisfied": 0.0, "violated": 0.0, "insufficient": 0.0},
                    },
                },
                "usage": {"input_tokens": 200, "output_tokens": 20},
            })))
            .mount(server)
            .await;
    }

    // -- request shape ---------------------------------------------------------

    #[test]
    fn serialized_request_matches_the_documented_shape() {
        let requirement = sample_requirement();
        let check = sample_check();
        let evidence = sample_evidence();

        let request = build_request("jev-latest", &requirement, &check, &evidence);
        let value = serde_json::to_value(&request).unwrap();

        assert_eq!(value["model"], "jev-latest");
        assert_eq!(
            value["state"],
            serde_json::json!({
                "requirement": {
                    "title": "Only Keystore signs JWTs",
                    "declared_in": "services/keystore/CHECKS.toml",
                },
                "check": {
                    "title": "Only Keystore imports the signing key",
                    "prompt": "Keystore alone imports the JWT signing key; no other service does.",
                },
                "evidence": [
                    {
                        "tool": "Read",
                        "input": {"file_path": "services/keystore/src/sign.rs"},
                        "output": "pub fn sign_jwt() { /* ... */ }\n",
                    },
                    {
                        "tool": "Grep",
                        "input": {"pattern": "sign_jwt", "path": "."},
                        "output": "services/keystore/src/sign.rs:1:pub fn sign_jwt() { /* ... */ }",
                    },
                ],
            })
        );

        let questions = &value["questions"];
        assert_eq!(questions["satisfied"]["type"], "noul");
        assert!(
            questions["satisfied"]["instructions"]
                .as_str()
                .unwrap()
                .contains("`evidence`")
        );
        assert_eq!(questions["reading"]["type"], "choice");
        assert_eq!(
            questions["reading"]["criteria"],
            serde_json::json!({
                "satisfied": "The evidence affirmatively demonstrates the check holds",
                "violated": "The evidence shows the check does not hold",
                "insufficient": "The evidence does not address the check either way",
            })
        );
    }

    /// A true snapshot: the request's serialized STRING (not just its
    /// `Value` tree) is byte-stable, both across repeated calls and against a
    /// fixed expected literal.
    ///
    /// The expected literal is a raw string, not a `serde_json::json!` value
    /// compared via `to_string` — confirmed empirically (see this ticket's
    /// review): `serde_json::Value`'s `Map` in this workspace's actual
    /// dependency resolution is **not** order-preserving despite `toml`'s
    /// `preserve_order` feature appearing in `cargo tree`'s output (that
    /// unification apparently doesn't reach this crate's own use of
    /// `serde_json::Value` — `assert_eq!` against another `json!` value would
    /// silently normalize away the exact ordering this test exists to pin
    /// down). `request.state` is built through exactly one
    /// `serde_json::to_value` hop (see `build_state`), so every object
    /// *inside* `state` serializes alphabetically by key, regardless of the
    /// Rust struct field order it came from — still perfectly deterministic
    /// (same bytes every run), just not the struct's declared order.
    /// `request.model`/`request.questions` never pass through `Value` at
    /// all (they're typed fields serialized directly), so `questions`
    /// preserves real insertion order — `satisfied` then `reading`, and
    /// `reading`'s `criteria` preserves `satisfied`, `violated`,
    /// `insufficient` — exactly item 4's ask, guaranteed by the `IndexMap`
    /// switch (a `HashMap` here would have randomized per process instead).
    #[test]
    fn serialized_request_is_byte_stable() {
        let requirement = sample_requirement();
        let check = sample_check();
        let evidence = sample_evidence();

        let request = build_request("jev-latest", &requirement, &check, &evidence);
        let actual = serde_json::to_string(&request).unwrap();

        // Repeating the call must reproduce the exact same bytes.
        let repeated = serde_json::to_string(&build_request(
            "jev-latest",
            &requirement,
            &check,
            &evidence,
        ))
        .unwrap();
        assert_eq!(actual, repeated);

        let expected = format!(
            r#"{{"state":{{"check":{{"prompt":"Keystore alone imports the JWT signing key; no other service does.","title":"Only Keystore imports the signing key"}},"evidence":[{{"input":{{"file_path":"services/keystore/src/sign.rs"}},"output":"pub fn sign_jwt() {{ /* ... */ }}\n","tool":"Read"}},{{"input":{{"path":".","pattern":"sign_jwt"}},"output":"services/keystore/src/sign.rs:1:pub fn sign_jwt() {{ /* ... */ }}","tool":"Grep"}}],"requirement":{{"declared_in":"services/keystore/CHECKS.toml","title":"Only Keystore signs JWTs"}}}},"model":"jev-latest","questions":{{"satisfied":{{"type":"noul","instructions":"{NOUL_INSTRUCTIONS}","criteria":{{"true":"The evidence affirmatively demonstrates the check is satisfied","false":"The evidence shows a violation, or is insufficient to demonstrate satisfaction"}}}},"reading":{{"type":"choice","instructions":"{CHOICE_INSTRUCTIONS}","criteria":{{"satisfied":"The evidence affirmatively demonstrates the check holds","violated":"The evidence shows the check does not hold","insufficient":"The evidence does not address the check either way"}}}}}}}}"#
        );
        assert_eq!(actual, expected);
    }

    #[test]
    fn negative_control_request_differs_only_in_evidence() {
        let requirement = sample_requirement();
        let check = sample_check();
        let evidence = sample_evidence();

        let with_evidence = build_request("jev-latest", &requirement, &check, &evidence);
        let control = build_request("jev-latest", &requirement, &check, &[]);

        let mut with_evidence_value = serde_json::to_value(&with_evidence).unwrap();
        let mut control_value = serde_json::to_value(&control).unwrap();

        // Both differ from the fixture only in `state.evidence`.
        assert_ne!(
            with_evidence_value["state"]["evidence"],
            control_value["state"]["evidence"]
        );
        assert_eq!(control_value["state"]["evidence"], serde_json::json!([]));

        // Neutralize that one field, and everything else must match exactly.
        with_evidence_value["state"]["evidence"] = serde_json::json!(null);
        control_value["state"]["evidence"] = serde_json::json!(null);
        assert_eq!(with_evidence_value, control_value);
    }

    /// The same check as `negative_control_request_differs_only_in_evidence`,
    /// but on the serialized STRING (see
    /// `serialized_request_is_byte_stable`'s docs on why that's a stronger
    /// check than comparing `Value`s): the two requests' bytes differ, but
    /// become byte-identical once `state.evidence` is neutralized in both.
    #[test]
    fn negative_control_request_string_differs_only_in_evidence() {
        let requirement = sample_requirement();
        let check = sample_check();
        let evidence = sample_evidence();

        let with_evidence = build_request("jev-latest", &requirement, &check, &evidence);
        let control = build_request("jev-latest", &requirement, &check, &[]);

        let with_evidence_str = serde_json::to_string(&with_evidence).unwrap();
        let control_str = serde_json::to_string(&control).unwrap();
        assert_ne!(with_evidence_str, control_str);

        let mut with_evidence_value = serde_json::to_value(&with_evidence).unwrap();
        let mut control_value = serde_json::to_value(&control).unwrap();
        with_evidence_value["state"]["evidence"] = serde_json::json!(null);
        control_value["state"]["evidence"] = serde_json::json!(null);
        assert_eq!(
            serde_json::to_string(&with_evidence_value).unwrap(),
            serde_json::to_string(&control_value).unwrap()
        );
    }

    // -- merging overlapping Reads ---------------------------------------------

    #[test]
    fn duplicate_whole_file_reads_collapse_to_one() {
        let evidence = vec![
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs"}),
                output: "fn a() {}\n".to_string(),
                windowed: false,
            },
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs"}),
                output: "fn a() {}\n".to_string(),
                windowed: false,
            },
        ];
        let merged = merge_overlapping_reads(&evidence);
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn windowed_read_dropped_when_a_whole_file_read_of_the_same_file_exists() {
        let evidence = vec![
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs", "offset": 0, "limit": 5}),
                output: "fn a() {\n".to_string(),
                windowed: true,
            },
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs"}),
                output: "fn a() {\n}\n".to_string(),
                windowed: false,
            },
        ];
        let merged = merge_overlapping_reads(&evidence);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].input, serde_json::json!({"file_path": "a.rs"}));
    }

    #[test]
    fn windowed_read_order_before_the_whole_file_read_is_still_dropped() {
        // Order in the evidence list must not matter: the whole-file read
        // arrives second here, and the windowed read is still dropped.
        let evidence = vec![
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs", "limit": 5}),
                output: "fn a() {\n".to_string(),
                windowed: true,
            },
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs"}),
                output: "fn a() {\n}\n".to_string(),
                windowed: false,
            },
        ];
        let merged = merge_overlapping_reads(&evidence);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].input.get("limit").is_none());
    }

    #[test]
    fn two_windowed_reads_of_the_same_file_are_never_spliced() {
        let evidence = vec![
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs", "offset": 0, "limit": 5}),
                output: "one\ntwo\n".to_string(),
                windowed: true,
            },
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs", "offset": 3, "limit": 5}),
                output: "four\nfive\n".to_string(),
                windowed: true,
            },
        ];
        let merged = merge_overlapping_reads(&evidence);
        assert_eq!(
            merged.len(),
            2,
            "no whole-file read exists to subsume either window"
        );
    }

    #[test]
    fn a_capped_read_marked_windowed_does_not_subsume_a_later_window_of_the_same_file() {
        // Simulates a Read that hit cersei's 2000-line default: its `input`
        // omits offset/limit entirely (so a heuristic based on `input` alone
        // would wrongly call it "whole-file"), but the caller marks it
        // `windowed: true` — exactly as `ReplayedCall::windowed` would,
        // since it did not see the whole file. See the module docs.
        let evidence = vec![
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "big.rs"}),
                output: "line\n".repeat(2000),
                windowed: true,
            },
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "big.rs", "offset": 2000, "limit": 500}),
                output: "line 2001\n".to_string(),
                windowed: true,
            },
        ];
        let merged = merge_overlapping_reads(&evidence);
        assert_eq!(
            merged.len(),
            2,
            "neither read covers the whole file, so neither subsumes the other"
        );
    }

    #[test]
    fn dot_slash_prefixed_and_bare_file_paths_are_treated_as_the_same_file() {
        let evidence = vec![
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "./a.rs"}),
                output: "fn a() {}\n".to_string(),
                windowed: false,
            },
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs"}),
                output: "fn a() {}\n".to_string(),
                windowed: false,
            },
        ];
        let merged = merge_overlapping_reads(&evidence);
        assert_eq!(merged.len(), 1, "`./a.rs` and `a.rs` name the same file");
    }

    #[test]
    fn reads_of_different_files_are_untouched() {
        let evidence = vec![
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "a.rs"}),
                output: "fn a() {}\n".to_string(),
                windowed: false,
            },
            Evidence {
                tool: ReadOnlyTool::Read,
                input: serde_json::json!({"file_path": "b.rs"}),
                output: "fn b() {}\n".to_string(),
                windowed: false,
            },
        ];
        let merged = merge_overlapping_reads(&evidence);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn grep_and_glob_entries_are_untouched() {
        let evidence = vec![
            Evidence {
                tool: ReadOnlyTool::Grep,
                input: serde_json::json!({"pattern": "x", "path": "."}),
                output: "a.rs:1:x".to_string(),
                windowed: false,
            },
            Evidence {
                tool: ReadOnlyTool::Grep,
                input: serde_json::json!({"pattern": "x", "path": "."}),
                output: "a.rs:1:x".to_string(),
                windowed: false,
            },
            Evidence {
                tool: ReadOnlyTool::Glob,
                input: serde_json::json!({"pattern": "*.rs", "path": "."}),
                output: "a.rs".to_string(),
                windowed: false,
            },
        ];
        let merged = merge_overlapping_reads(&evidence);
        assert_eq!(merged.len(), 3, "only Read is ever merged/dropped");
    }

    // -- threshold boundary -----------------------------------------------------

    #[tokio::test]
    async fn noul_equal_to_threshold_is_satisfied() {
        let server = MockServer::start().await;
        mock_answer(&server, 0.75, "satisfied", 0.9).await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::Satisfied { noul, model, .. } => {
                assert_eq!(noul, 0.75);
                assert_eq!(model, "jev-1.13.0");
            }
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn noul_just_below_threshold_is_not_verified() {
        let server = MockServer::start().await;
        mock_answer(&server, 0.7499, "insufficient", 0.5).await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        assert!(matches!(decision, JevDecision::NotVerified { .. }));
    }

    // -- Choice never gates ------------------------------------------------------

    #[tokio::test]
    async fn choice_never_gates_the_decision_even_when_contradicting() {
        let server = MockServer::start().await;
        // High Noul (well above threshold) paired with a Choice that says
        // the check is violated.
        mock_answer(&server, 0.95, "violated", 0.99).await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::Satisfied { choice, .. } => {
                assert_eq!(
                    choice.map(|c| c.reading),
                    Some(Reading::Violated),
                    "the contradicting Choice is still surfaced"
                );
            }
            other => panic!("expected Satisfied (Noul gates alone), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn choice_never_gates_the_decision_the_other_way_either() {
        let server = MockServer::start().await;
        // Low Noul (well below threshold) paired with a Choice that says
        // the check is satisfied.
        mock_answer(&server, 0.1, "satisfied", 0.99).await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::NotVerified { choice, .. } => {
                assert_eq!(choice.map(|c| c.reading), Some(Reading::Satisfied));
            }
            other => panic!("expected NotVerified (Noul gates alone), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_choice_answer_does_not_error_the_decision() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {
                    NOUL_QUESTION_ID: {"type": "noul", "noul": 0.9},
                },
                "usage": {"input_tokens": 100, "output_tokens": 10},
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::Satisfied { choice, .. } => assert_eq!(choice, None),
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_noul_answer_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {
                    CHOICE_QUESTION_ID: {
                        "type": "choice", "choice": "satisfied", "confidence": 0.5,
                        "probabilities": {"satisfied": 0.5, "violated": 0.3, "insufficient": 0.2},
                    },
                },
                "usage": {"input_tokens": 100, "output_tokens": 10},
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let err = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::Transport(_)));
    }

    // -- Choice probabilities ----------------------------------------------------

    async fn mock_choice_probabilities(server: &MockServer, probabilities: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {
                    NOUL_QUESTION_ID: {"type": "noul", "noul": 0.6},
                    CHOICE_QUESTION_ID: {
                        "type": "choice", "choice": "satisfied", "confidence": 0.4,
                        "probabilities": probabilities,
                    },
                },
                "usage": {"input_tokens": 100, "output_tokens": 10},
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn choice_outcome_carries_every_reading_probability() {
        let server = MockServer::start().await;
        mock_choice_probabilities(
            &server,
            serde_json::json!({"satisfied": 0.6, "violated": 0.1, "insufficient": 0.3}),
        )
        .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();
        let JevDecision::NotVerified { choice, .. } = decision else {
            panic!("expected NotVerified, got {decision:?}");
        };
        let choice = choice.expect("well-formed Choice answer");
        assert_eq!(choice.confidence, 0.4);
        assert_eq!(
            choice.probabilities,
            Some(ReadingProbabilities {
                satisfied: 0.6,
                violated: 0.1,
                insufficient: 0.3,
            })
        );
    }

    #[tokio::test]
    async fn partial_choice_probabilities_keep_the_reading_but_record_none() {
        let server = MockServer::start().await;
        mock_choice_probabilities(
            &server,
            serde_json::json!({"satisfied": 0.6, "violated": 0.4}),
        )
        .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();
        let JevDecision::NotVerified { choice, .. } = decision else {
            panic!("expected NotVerified, got {decision:?}");
        };
        let choice = choice.expect("reading still parses");
        assert_eq!(choice.reading, Reading::Satisfied);
        assert_eq!(choice.probabilities, None);
    }

    // -- Noul range validation ---------------------------------------------------

    #[tokio::test]
    async fn noul_above_one_is_an_error_not_a_silent_satisfied() {
        let server = MockServer::start().await;
        mock_answer(&server, 1.2, "satisfied", 0.9).await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let err = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::Transport(_)));
    }

    #[tokio::test]
    async fn noul_below_zero_is_an_error() {
        let server = MockServer::start().await;
        mock_answer(&server, -0.1, "insufficient", 0.9).await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let err = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::Transport(_)));
    }

    #[test]
    fn valid_probability_rejects_out_of_range_and_non_finite_values() {
        assert!(valid_probability(0.0));
        assert!(valid_probability(1.0));
        assert!(valid_probability(0.5));
        assert!(!valid_probability(1.2));
        assert!(!valid_probability(-0.1));
        assert!(!valid_probability(f64::NAN));
        assert!(!valid_probability(f64::INFINITY));
        assert!(!valid_probability(f64::NEG_INFINITY));
    }

    // -- a malformed/unusual Choice answer never blocks the Noul decision --------

    #[tokio::test]
    async fn choice_missing_confidence_does_not_block_the_noul_decision() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {
                    NOUL_QUESTION_ID: {"type": "noul", "noul": 0.9},
                    // Missing `confidence`/`probabilities`.
                    CHOICE_QUESTION_ID: {"type": "choice", "choice": "satisfied"},
                },
                "usage": {"input_tokens": 100, "output_tokens": 10},
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::Satisfied { noul, choice, .. } => {
                assert_eq!(noul, 0.9);
                assert_eq!(choice, None);
            }
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn choice_answer_with_wrong_type_does_not_block_the_noul_decision() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {
                    NOUL_QUESTION_ID: {"type": "noul", "noul": 0.9},
                    // A Noul-shaped answer under the Choice question's id.
                    CHOICE_QUESTION_ID: {"type": "noul", "noul": 0.5},
                },
                "usage": {"input_tokens": 100, "output_tokens": 10},
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::Satisfied { choice, .. } => assert_eq!(choice, None),
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn choice_answer_with_unknown_type_does_not_block_the_noul_decision() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {
                    NOUL_QUESTION_ID: {"type": "noul", "noul": 0.9},
                    // Not a recognized answer `type` at all.
                    CHOICE_QUESTION_ID: {"type": "score", "score": 3.0},
                },
                "usage": {"input_tokens": 100, "output_tokens": 10},
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::Satisfied { choice, .. } => assert_eq!(choice, None),
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn extra_unknown_answer_id_does_not_block_the_noul_decision() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {
                    NOUL_QUESTION_ID: {"type": "noul", "noul": 0.9},
                    // No `reading` entry at all, but an entirely unrelated
                    // extra id with an unrecognized shape.
                    "mystery": {"type": "score", "score": 3.0},
                },
                "usage": {"input_tokens": 100, "output_tokens": 10},
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::Satisfied { noul, choice, .. } => {
                assert_eq!(noul, 0.9);
                assert_eq!(choice, None);
            }
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn choice_confidence_out_of_range_degrades_to_none() {
        let server = MockServer::start().await;
        mock_answer(&server, 0.9, "satisfied", 1.5).await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::Satisfied { choice, .. } => assert_eq!(choice, None),
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    // -- budget -------------------------------------------------------------------

    #[tokio::test]
    async fn over_budget_evidence_makes_zero_http_calls() {
        let server = MockServer::start().await;
        // No mock mounted at all: any HTTP call would fail to match and
        // panic wiremock's default "no matching mock" handler, so this also
        // fails loudly if `verify` ever calls the API here.
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);

        let huge_evidence = vec![Evidence {
            tool: ReadOnlyTool::Read,
            input: serde_json::json!({"file_path": "huge.rs"}),
            output: "x".repeat(200_000),
            windowed: false,
        }];
        let (requirement, check) = (sample_requirement(), sample_check());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &huge_evidence))
            .await
            .unwrap();

        assert_eq!(decision, JevDecision::OverBudget);
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "over-budget evidence must never reach the network"
        );
    }

    #[tokio::test]
    async fn small_evidence_is_under_budget() {
        let estimate = estimate_tokens(
            &build_state(&sample_requirement(), &sample_check(), &sample_evidence()),
            &{
                let mut q = IndexMap::new();
                q.insert(NOUL_QUESTION_ID.to_string(), noul_question());
                q.insert(CHOICE_QUESTION_ID.to_string(), choice_question());
                q
            },
        );
        assert!(estimate < TOKEN_BUDGET);
    }

    #[tokio::test]
    async fn invalid_422_naming_the_context_limit_maps_to_over_budget() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "detail": [{
                    "loc": ["body", "state"],
                    "msg": "request exceeds the model's context limit of 32000 tokens",
                    "type": "value_error",
                }],
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        assert_eq!(decision, JevDecision::OverBudget);
    }

    #[tokio::test]
    async fn invalid_422_unrelated_to_the_context_limit_stays_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "detail": [{
                    "loc": ["body", "questions"],
                    "msg": "field required",
                    "type": "missing",
                }],
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let err = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap_err();

        assert!(matches!(err, JevError::Invalid { .. }));
    }

    // -- transport failures --------------------------------------------------------

    #[tokio::test]
    async fn transport_failure_is_an_error_distinct_from_not_verified() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(529))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let err = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap_err();

        assert!(matches!(err, JevError::Exhausted { .. }));
    }

    // -- resolved model id -------------------------------------------------------

    #[tokio::test]
    async fn decision_carries_the_resolved_model_id_not_the_configured_alias() {
        let server = MockServer::start().await;
        mock_answer(&server, 0.9, "satisfied", 0.9).await;
        let client = JevClient::new(server.uri()).unwrap();
        let mut cfg = config(0.75);
        cfg.model = "jev-latest".to_string();
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let decision = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap();

        match decision {
            JevDecision::Satisfied { model, .. } => assert_eq!(model, "jev-1.13.0"),
            other => panic!("expected Satisfied, got {other:?}"),
        }
    }

    // -- agreement ------------------------------------------------------------------

    #[test]
    fn agreement_pass_expected_boundaries_and_middle() {
        // threshold = 0.75, low = 0.25.
        assert_eq!(agreement(Expected::Pass, 0.75, 0.75), Agreement::Agrees);
        assert_eq!(agreement(Expected::Pass, 1.0, 0.75), Agreement::Agrees);
        assert_eq!(agreement(Expected::Pass, 0.25, 0.75), Agreement::Disagrees);
        assert_eq!(agreement(Expected::Pass, 0.0, 0.75), Agreement::Disagrees);
        assert_eq!(agreement(Expected::Pass, 0.5, 0.75), Agreement::Uncertain);
    }

    #[test]
    fn agreement_fail_expected_boundaries_and_middle() {
        assert_eq!(agreement(Expected::Fail, 0.25, 0.75), Agreement::Agrees);
        assert_eq!(agreement(Expected::Fail, 0.0, 0.75), Agreement::Agrees);
        assert_eq!(agreement(Expected::Fail, 0.75, 0.75), Agreement::Disagrees);
        assert_eq!(agreement(Expected::Fail, 1.0, 0.75), Agreement::Disagrees);
        assert_eq!(agreement(Expected::Fail, 0.5, 0.75), Agreement::Uncertain);
    }

    #[test]
    fn agreement_negative_control_uses_the_fail_side() {
        // The empty-evidence negative control is scored exactly like a
        // genuine failure expectation: low noul agrees, high noul disagrees.
        assert_eq!(agreement(Expected::Fail, 0.1, 0.75), Agreement::Agrees);
        assert_eq!(agreement(Expected::Fail, 0.9, 0.75), Agreement::Disagrees);
    }

    #[test]
    fn agreement_bands_crossing_below_half_threshold_is_uncertain() {
        // threshold = 0.3, low = 0.7: the bands cross (low > threshold). A
        // noul that satisfies both the "agrees" and "disagrees" conditions
        // at once is treated as Uncertain rather than picking a side — see
        // `agreement`'s docs.
        assert_eq!(agreement(Expected::Pass, 0.5, 0.3), Agreement::Uncertain);
        assert_eq!(agreement(Expected::Fail, 0.5, 0.3), Agreement::Uncertain);
        // Genuinely one-sided values are unaffected by the crossing.
        assert_eq!(agreement(Expected::Pass, 1.0, 0.3), Agreement::Agrees);
        assert_eq!(agreement(Expected::Fail, 0.0, 0.3), Agreement::Agrees);
    }

    #[test]
    fn agreement_at_threshold_one_half_the_bands_touch_and_the_boundary_is_uncertain() {
        // threshold = 0.5, low = 0.5: the bands touch exactly rather than
        // cross — the boundary noul satisfies both conditions at once.
        assert_eq!(agreement(Expected::Pass, 0.5, 0.5), Agreement::Uncertain);
        assert_eq!(agreement(Expected::Fail, 0.5, 0.5), Agreement::Uncertain);
        assert_eq!(agreement(Expected::Pass, 0.6, 0.5), Agreement::Agrees);
        assert_eq!(agreement(Expected::Pass, 0.4, 0.5), Agreement::Disagrees);
        assert_eq!(agreement(Expected::Fail, 0.4, 0.5), Agreement::Agrees);
        assert_eq!(agreement(Expected::Fail, 0.6, 0.5), Agreement::Disagrees);
    }

    #[test]
    fn agreement_at_threshold_one_only_a_perfect_noul_agrees() {
        // threshold = 1.0, low = 0.0: the widest possible Uncertain band —
        // only an exact 1.0 (Pass) or exact 0.0 (Fail) agrees.
        assert_eq!(agreement(Expected::Pass, 1.0, 1.0), Agreement::Agrees);
        assert_eq!(agreement(Expected::Pass, 0.0, 1.0), Agreement::Disagrees);
        assert_eq!(agreement(Expected::Pass, 0.5, 1.0), Agreement::Uncertain);
        assert_eq!(agreement(Expected::Fail, 0.0, 1.0), Agreement::Agrees);
        assert_eq!(agreement(Expected::Fail, 1.0, 1.0), Agreement::Disagrees);
        assert_eq!(agreement(Expected::Fail, 0.5, 1.0), Agreement::Uncertain);
    }

    // -- context-limit wording heuristic -------------------------------------------

    #[test]
    fn context_limit_wording_is_recognized_in_the_documented_shape() {
        let body = serde_json::json!({
            "detail": [{
                "loc": ["body", "state"],
                "msg": "input exceeds the maximum context length: too many tokens",
                "type": "value_error",
            }],
        })
        .to_string();
        assert!(looks_like_context_limit(&body));
    }

    #[test]
    fn context_limit_wording_falls_back_to_raw_body_text() {
        assert!(looks_like_context_limit(
            "413: request exceeds the token/context limit"
        ));
    }

    #[test]
    fn unrelated_validation_error_is_not_recognized_as_a_context_limit() {
        let body = serde_json::json!({
            "detail": [{"loc": ["body", "model"], "msg": "field required", "type": "missing"}],
        })
        .to_string();
        assert!(!looks_like_context_limit(&body));
    }

    /// Near misses: each mentions "token" or "limit" (the old, looser
    /// heuristic's trigger words) without being about the context window at
    /// all. A genuine `Invalid` on one of these must stay `Err`, never
    /// silently downgrade to `OverBudget` — see the module docs.
    #[test]
    fn near_miss_api_token_wording_is_not_a_context_limit() {
        let body = serde_json::json!({
            "detail": [{"loc": ["header", "authorization"], "msg": "API token invalid", "type": "value_error"}],
        })
        .to_string();
        assert!(!looks_like_context_limit(&body));
    }

    #[test]
    fn near_miss_criteria_option_limit_wording_is_not_a_context_limit() {
        let body = serde_json::json!({
            "detail": [{
                "loc": ["body", "questions", "reading", "criteria"],
                "msg": "criteria limit of 255 options exceeded",
                "type": "value_error",
            }],
        })
        .to_string();
        assert!(!looks_like_context_limit(&body));
    }

    #[tokio::test]
    async fn invalid_422_with_near_miss_api_token_wording_stays_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "detail": [{
                    "loc": ["header", "authorization"],
                    "msg": "API token invalid",
                    "type": "value_error",
                }],
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let err = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::Invalid { .. }));
    }

    #[tokio::test]
    async fn invalid_422_with_near_miss_criteria_limit_wording_stays_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "detail": [{
                    "loc": ["body", "questions", "reading", "criteria"],
                    "msg": "criteria limit of 255 options exceeded",
                    "type": "value_error",
                }],
            })))
            .mount(&server)
            .await;
        let client = JevClient::new(server.uri()).unwrap();
        let cfg = config(0.75);
        let (requirement, check, evidence) =
            (sample_requirement(), sample_check(), sample_evidence());

        let err = with_api_key(|| verify(&client, &cfg, &requirement, &check, &evidence))
            .await
            .unwrap_err();
        assert!(matches!(err, JevError::Invalid { .. }));
    }
}
