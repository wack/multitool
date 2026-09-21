//! Typed failures from a [`super::client::JevClient`] request.
//!
//! Every TypeSafe SystemOne failure resolves to exactly one of these variants
//! (<https://docs.typesafe.ai/api.md#error-responses>), so a caller can react
//! differently to each rather than pattern-matching HTTP status codes.

use miette::Diagnostic;
use thiserror::Error;

/// The environment variable carrying the TypeSafe API key. Resolved lazily by
/// [`super::client::JevClient`] on the first request — never at construction,
/// and never logged.
pub const TYPESAFE_API_KEY_VAR: &str = "TYPESAFE_API_KEY";

/// A failed Jev/TypeSafe SystemOne request.
#[derive(Debug, Error, Diagnostic)]
pub enum JevError {
    /// `TYPESAFE_API_KEY` is unset (or empty) when a request was attempted.
    /// Constructing a [`super::client::JevClient`] never triggers this — only
    /// making a request does, so a run that never calls Jev needs no key.
    #[error("{TYPESAFE_API_KEY_VAR} is not set; export it before making a Jev request")]
    #[diagnostic(
        code(jev::missing_api_key),
        help("set {TYPESAFE_API_KEY_VAR} in the environment")
    )]
    MissingApiKey,

    /// `401 Unauthorized`: the API key was missing or invalid.
    #[error("TypeSafe rejected the request: missing or invalid API key")]
    #[diagnostic(
        code(jev::unauthorized),
        help("check that {TYPESAFE_API_KEY_VAR} is a valid TypeSafe API key")
    )]
    Unauthorized,

    /// `422 Unprocessable Entity`: the request body failed server-side
    /// validation. Carries the raw response body for diagnosis.
    #[error("TypeSafe rejected the request as invalid: {body}")]
    #[diagnostic(code(jev::invalid))]
    Invalid { body: String },

    /// `429`/`529` (rate-limited / overloaded) persisted through every retry
    /// in the backoff budget.
    #[error(
        "TypeSafe requests exhausted the retry budget after {attempts} attempt(s); last status {status}"
    )]
    #[diagnostic(
        code(jev::exhausted),
        help("TypeSafe is rate-limiting or overloaded; try again later")
    )]
    Exhausted { status: u16, attempts: u32 },

    /// A connection failure, a timeout, or a response whose body was not the
    /// documented shape.
    #[error("TypeSafe request failed: {0}")]
    #[diagnostic(code(jev::transport))]
    Transport(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_api_key_names_the_variable() {
        let err = JevError::MissingApiKey.to_string();
        assert!(err.contains(TYPESAFE_API_KEY_VAR), "got: {err}");
    }

    #[test]
    fn invalid_carries_the_response_body() {
        let err = JevError::Invalid {
            body: "missing required field `questions`".to_string(),
        };
        assert!(err.to_string().contains("missing required field"));
    }
}
