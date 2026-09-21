//! Wire types for TypeSafe's `POST /v1/systemone` endpoint
//! (<https://docs.typesafe.ai/api.md>).
//!
//! `Question`/`Answer` are modeled as enums tagged by TypeSafe's own `"type"`
//! discriminant. `Noul` (MULTI-1819) and `Choice` (MULTI-1823, added to
//! support `crate::checks::jev::verify`'s single request carrying both a Noul
//! `satisfied` question and a record-only Choice `reading` question over the
//! same state — see <https://docs.typesafe.ai/primitives/choice.md>) are both
//! implemented; `Score` (<https://docs.typesafe.ai/primitives/score.md>) is
//! not, since nothing in this milestone asks one.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A request to `POST {base_url}/v1/systemone`
/// (<https://docs.typesafe.ai/api.md#request-schema>).
#[derive(Debug, Clone, Serialize)]
pub struct SystemOneRequest {
    /// The content to evaluate: a plain string, or structured data (object /
    /// array) — chat logs, records, or the current state of the caller's
    /// application.
    pub state: serde_json::Value,
    /// The Jev model ID or alias to run (e.g. `jev-latest`).
    pub model: String,
    /// Named questions to ask over `state`, keyed by a caller-chosen id. The
    /// same id keys the corresponding entry in [`SystemOneResponse::answers`].
    pub questions: HashMap<String, Question>,
}

/// One question in a [`SystemOneRequest`]. Internally tagged by TypeSafe's
/// `"type"` field (e.g. `{"type": "noul", "instructions": ..., "criteria": ...}`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    Noul(NoulQuestion),
    Choice(ChoiceQuestion),
}

/// A Noul (yes/no) question (<https://docs.typesafe.ai/primitives/noul.md>):
/// returns the probability that the answer is yes.
#[derive(Debug, Clone, Serialize)]
pub struct NoulQuestion {
    /// The yes/no question or statement to evaluate against `state`.
    pub instructions: String,
    /// Optional semantic clarification of what counts as yes/no.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria: Option<NoulCriteria>,
}

/// `criteria.true` / `criteria.false` clarifying a [`NoulQuestion`]'s yes/no
/// semantics. Both fields are named after Rust keywords, hence the renames.
#[derive(Debug, Clone, Serialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub when_true: String,
    #[serde(rename = "false")]
    pub when_false: String,
}

/// A Choice question (<https://docs.typesafe.ai/primitives/choice.md>):
/// selects one option from a fixed set. Unlike [`NoulQuestion`], `criteria`
/// is required — it's how the option set itself is expressed, not just a
/// clarification of it: `{"type": "choice", "instructions": ..., "criteria":
/// {"option_a": "description", "option_b": "description"}}`, up to 255
/// options.
#[derive(Debug, Clone, Serialize)]
pub struct ChoiceQuestion {
    /// The question being asked (e.g. "Which team should handle this?").
    pub instructions: String,
    /// Option name -> description, up to 255 entries
    /// (<https://docs.typesafe.ai/primitives/choice.md>).
    pub criteria: HashMap<String, String>,
}

/// A successful `200` response from `POST /v1/systemone`
/// (<https://docs.typesafe.ai/api.md#response-schema>).
#[derive(Debug, Clone, Deserialize)]
pub struct SystemOneResponse {
    /// The concrete model that actually answered (e.g. `jev-1.13.0`), which
    /// may differ from the requested alias (e.g. `jev-latest`).
    pub model: String,
    /// One answer per requested question, keyed by the same id.
    pub answers: HashMap<String, Answer>,
    /// Token accounting for this request.
    pub usage: Usage,
}

/// One answer in a [`SystemOneResponse`]. Tagged by TypeSafe's `"type"` field,
/// mirroring [`Question`] — but deserialized by hand (see the `Deserialize`
/// impl below) rather than via `#[serde(tag = "type")]`.
#[derive(Debug, Clone)]
pub enum Answer {
    Noul(NoulAnswer),
    Choice(ChoiceAnswer),
}

/// Hand-rolled rather than `#[serde(tag = "type")]`: this crate also depends
/// (transitively, via `bigdecimal`'s `serde-json` feature, unrelated to Jev)
/// on serde_json's `arbitrary_precision`, which represents every JSON number
/// as a single-key map internally. serde's derived internally-tagged-enum
/// deserializer buffers content through that same map-shaped representation,
/// and the two interact badly: a request through `serde_json::from_str`
/// reports a leaf number field (`noul`) as "invalid type: map, expected f64"
/// even though the JSON itself is well-formed (confirmed with a minimal
/// reproduction outside this crate — the derive-generated code is fine,
/// `arbitrary_precision` is the trigger). Deserializing to a
/// [`serde_json::Value`] first and re-dispatching on `"type"` — exactly what
/// `serde_json::from_value` does internally — sidesteps the derive macro's
/// buffering path entirely and is unaffected.
impl<'de> Deserialize<'de> for Answer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let value = serde_json::Value::deserialize(deserializer)?;
        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| D::Error::missing_field("type"))?
            .to_string();
        match kind.as_str() {
            "noul" => serde_json::from_value(value)
                .map(Answer::Noul)
                .map_err(D::Error::custom),
            "choice" => serde_json::from_value(value)
                .map(Answer::Choice)
                .map_err(D::Error::custom),
            other => Err(D::Error::unknown_variant(other, &["noul", "choice"])),
        }
    }
}

/// A Noul answer: the probability (`0.0..=1.0`) that the answer is yes.
#[derive(Debug, Clone, Deserialize)]
pub struct NoulAnswer {
    pub noul: f64,
}

/// A Choice answer (<https://docs.typesafe.ai/primitives/choice.md>): the
/// option with the highest probability, the full probability distribution
/// over every option named in the question's `criteria`, and a derived
/// `confidence` (<https://docs.typesafe.ai/confidence.md>) — "the answer's
/// `confidence` property collapses that shape into a single number from 0 to
/// 1, so you can threshold on it without doing the math yourself."
#[derive(Debug, Clone, Deserialize)]
pub struct ChoiceAnswer {
    /// The selected option (the key with the highest probability).
    pub choice: String,
    /// `0.0..=1.0`; derived from how concentrated `probabilities` is on
    /// [`Self::choice`] — a flatter distribution means lower confidence.
    pub confidence: f64,
    /// Every option's probability, summing to `1.0`.
    pub probabilities: HashMap<String, f64>,
}

/// Token usage for one `/v1/systemone` request. `output_tokens` is part of the
/// documented response but is intentionally not modeled here: nothing in this
/// milestone reads it, and adding an unread field back is a one-line change
/// when a caller needs it (see `usage.input_tokens` logging in
/// [`super::client`]).
#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noul_question_serializes_to_the_documented_shape() {
        let question = Question::Noul(NoulQuestion {
            instructions: "Does this convey urgency?".to_string(),
            criteria: Some(NoulCriteria {
                when_true: "Explicitly time-sensitive".to_string(),
                when_false: "No urgency expressed".to_string(),
            }),
        });
        let value = serde_json::to_value(&question).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "noul",
                "instructions": "Does this convey urgency?",
                "criteria": {
                    "true": "Explicitly time-sensitive",
                    "false": "No urgency expressed",
                },
            })
        );
    }

    #[test]
    fn noul_question_omits_criteria_when_absent() {
        let question = Question::Noul(NoulQuestion {
            instructions: "Is this urgent?".to_string(),
            criteria: None,
        });
        let value = serde_json::to_value(&question).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"type": "noul", "instructions": "Is this urgent?"})
        );
    }

    #[test]
    fn system_one_response_deserializes_the_documented_example() {
        // The example response from https://docs.typesafe.ai/primitives/noul.md
        let raw = serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {
                "question_id": {"type": "noul", "noul": 0.99},
            },
            "usage": {"input_tokens": 360, "output_tokens": 39},
        });
        let response: SystemOneResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.usage.input_tokens, 360);
        match response.answers.get("question_id") {
            Some(Answer::Noul(NoulAnswer { noul })) => assert_eq!(*noul, 0.99),
            other => panic!("expected a Noul answer, got {other:?}"),
        }
    }

    #[test]
    fn choice_question_serializes_to_the_documented_shape() {
        // The example request from https://docs.typesafe.ai/primitives/choice.md
        let mut criteria = HashMap::new();
        criteria.insert(
            "returns".to_string(),
            "Exchanges, wrong or damaged items".to_string(),
        );
        criteria.insert(
            "shipping".to_string(),
            "Delivery status, delays, lost packages".to_string(),
        );
        criteria.insert(
            "billing".to_string(),
            "Charges, invoices, payment problems".to_string(),
        );
        let question = Question::Choice(ChoiceQuestion {
            instructions: "Which team should handle this?".to_string(),
            criteria,
        });
        let value = serde_json::to_value(&question).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "type": "choice",
                "instructions": "Which team should handle this?",
                "criteria": {
                    "returns": "Exchanges, wrong or damaged items",
                    "shipping": "Delivery status, delays, lost packages",
                    "billing": "Charges, invoices, payment problems",
                },
            })
        );
    }

    #[test]
    fn choice_answer_deserializes_the_documented_example() {
        // The example response from https://docs.typesafe.ai/primitives/choice.md
        let raw = serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {
                "department": {
                    "type": "choice",
                    "choice": "returns",
                    "confidence": 1.0,
                    "probabilities": {"shipping": 0.0, "returns": 1.0, "billing": 0.0},
                },
            },
            "usage": {"input_tokens": 328, "output_tokens": 34},
        });
        let response: SystemOneResponse = serde_json::from_value(raw).unwrap();
        match response.answers.get("department") {
            Some(Answer::Choice(answer)) => {
                assert_eq!(answer.choice, "returns");
                assert_eq!(answer.confidence, 1.0);
                assert_eq!(answer.probabilities.get("returns"), Some(&1.0));
            }
            other => panic!("expected a Choice answer, got {other:?}"),
        }
    }

    #[test]
    fn unknown_answer_type_is_rejected() {
        let raw = serde_json::json!({
            "model": "jev-1.13.0",
            "answers": {"q": {"type": "score", "score": 3.0}},
            "usage": {"input_tokens": 1, "output_tokens": 1},
        });
        let err = serde_json::from_value::<SystemOneResponse>(raw).unwrap_err();
        assert!(err.to_string().contains("score"), "got: {err}");
    }
}
