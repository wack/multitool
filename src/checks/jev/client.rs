//! The TypeSafe SystemOne HTTP client (<https://docs.typesafe.ai/api.md>).
//!
//! `POST {base_url}/v1/systemone` with `Authorization: Bearer <key>`. The API
//! key is resolved **lazily**, straight from [`TYPESAFE_API_KEY_VAR`], on the
//! first request a [`JevClient`] actually sends — never at construction, and
//! never logged — so a `multi check` run whose every check settles from cache
//! never needs the key to be set.
//!
//! `429` (rate limited) and `529` (overloaded) responses are retried with
//! exponential backoff (<https://docs.typesafe.ai/sdk/python/api/retries.md>
//! documents TypeSafe's own SDK defaults: 2 retries, 0.5s initial delay,
//! doubling, capped at 5s — mirrored here since we have no other basis to pick
//! different numbers). Every other failure maps to one [`JevError`] variant.

use std::time::Duration;

use crate::checks::config::JevConfig;

use super::error::{JevError, TYPESAFE_API_KEY_VAR};
use super::types::{SystemOneRequest, SystemOneResponse};

/// Per-request timeout. TypeSafe questions are small (state capped at 32k
/// tokens; see <https://docs.typesafe.ai/models.md>) and System One is a fast
/// model, so a generous-but-bounded timeout catches a hung connection without
/// being trigger-happy on a slow network.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How many attempts a request gets in total (the first try, plus retries) on
/// `429`/`529` before giving up as [`JevError::Exhausted`].
const MAX_ATTEMPTS: u32 = 3;
/// The delay before the first retry.
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);
/// The backoff delay is doubled after every retry, capped at this value.
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Cap on the response body embedded in a [`JevError::Transport`] diagnostic
/// for a status code we don't otherwise special-case (e.g. an upstream
/// proxy's HTML error page can be arbitrarily large).
const MAX_TRANSPORT_BODY_BYTES: usize = 512;

/// Truncate `body` to [`MAX_TRANSPORT_BODY_BYTES`] on a char boundary,
/// appending a marker noting how many bytes were dropped.
fn truncate_body(body: &str) -> String {
    if body.len() <= MAX_TRANSPORT_BODY_BYTES {
        return body.to_string();
    }
    let mut end = MAX_TRANSPORT_BODY_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated {} bytes]", &body[..end], body.len() - end)
}

/// A ready-to-use TypeSafe SystemOne client. Cheap to construct: building one
/// opens no connection and touches no credential (see the module docs).
pub struct JevClient {
    http: reqwest::Client,
    /// The TypeSafe SystemOne API origin, e.g. `https://api.typesafe.ai`. The
    /// client joins this with the `/v1/systemone` path per request.
    base_url: String,
}

impl JevClient {
    /// Construct a client for TypeSafe's API at `base_url`. Never reads
    /// `TYPESAFE_API_KEY` — that happens lazily in [`Self::ask`].
    pub fn new(base_url: impl Into<String>) -> Result<Self, JevError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| JevError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base_url: base_url.into(),
        })
    }

    /// Construct a client from a resolved [`JevConfig`] (`checks.jev.base_url`,
    /// defaulted to TypeSafe's production origin — see
    /// `crate::checks::config::jev::resolve_jev`).
    pub fn from_config(config: &JevConfig) -> Result<Self, JevError> {
        Self::new(config.base_url.clone())
    }

    /// Resolve the API key from the environment. Called once per request, not
    /// at construction — see the module docs.
    fn api_key() -> Result<String, JevError> {
        std::env::var(TYPESAFE_API_KEY_VAR)
            .ok()
            .filter(|key| !key.is_empty())
            .ok_or(JevError::MissingApiKey)
    }

    /// Send `request` to `POST {base_url}/v1/systemone`, retrying `429`/`529`
    /// with exponential backoff up to [`MAX_ATTEMPTS`] total tries.
    pub async fn ask(&self, request: &SystemOneRequest) -> Result<SystemOneResponse, JevError> {
        let api_key = Self::api_key()?;
        let url = format!("{}/v1/systemone", self.base_url.trim_end_matches('/'));

        let mut backoff = INITIAL_BACKOFF;
        for attempt in 1..=MAX_ATTEMPTS {
            let response = self
                .http
                .post(&url)
                .bearer_auth(&api_key)
                .json(request)
                .send()
                .await
                .map_err(|e| JevError::Transport(e.to_string()))?;

            let status = response.status();
            if status.is_success() {
                let body = response
                    .text()
                    .await
                    .map_err(|e| JevError::Transport(e.to_string()))?;
                let parsed: SystemOneResponse = serde_json::from_str(&body)
                    .map_err(|e| JevError::Transport(format!("malformed response body: {e}")))?;
                tracing::debug!(
                    model = %parsed.model,
                    input_tokens = parsed.usage.input_tokens,
                    "jev systemone response",
                );
                return Ok(parsed);
            }

            match status.as_u16() {
                401 => return Err(JevError::Unauthorized),
                422 => {
                    let body = response.text().await.unwrap_or_default();
                    return Err(JevError::Invalid { body });
                }
                429 | 529 => {
                    if attempt == MAX_ATTEMPTS {
                        return Err(JevError::Exhausted {
                            status: status.as_u16(),
                            attempts: attempt,
                        });
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
                _ => {
                    let body = response.text().await.unwrap_or_default();
                    return Err(JevError::Transport(format!(
                        "unexpected status {status}: {}",
                        truncate_body(&body)
                    )));
                }
            }
        }
        unreachable!("the loop above always returns on its final attempt");
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::checks::jev::types::{NoulCriteria, NoulQuestion, Question};

    /// A minimal single-question Noul request, matching
    /// https://docs.typesafe.ai/api.md's example request.
    fn sample_request() -> SystemOneRequest {
        let mut questions = IndexMap::new();
        questions.insert(
            "is_urgent".to_string(),
            Question::Noul(NoulQuestion {
                instructions: "Does this convey urgency?".to_string(),
                criteria: Some(NoulCriteria {
                    when_true: "Explicitly time-sensitive".to_string(),
                    when_false: "No urgency expressed".to_string(),
                }),
            }),
        );
        SystemOneRequest {
            state: serde_json::json!("Help! My payouts have been failing for 3 days."),
            model: "jev-latest".to_string(),
            questions,
        }
    }

    /// Serializes tests that mutate `TYPESAFE_API_KEY_VAR`. Env vars are
    /// process-global; `cargo nextest run` (this repo's canonical runner, see
    /// `CLAUDE.md`) isolates every test into its own process, so this only
    /// matters for a plain `cargo test`, where tests run as threads sharing
    /// one process. An async-aware `Mutex` (not `std::sync::Mutex`) because the
    /// guard must stay held across the awaited `body()` call.
    static API_KEY_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Run async `body` with `TYPESAFE_API_KEY` set to `key` (or unset, for
    /// `None`) for its duration, restoring whatever was there before.
    async fn with_api_key<F, Fut, T>(key: Option<&str>, body: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let _guard = API_KEY_ENV_LOCK.lock().await;
        let previous = std::env::var(TYPESAFE_API_KEY_VAR).ok();
        // SAFETY: env mutation races with other threads reading/writing env
        // vars are the hazard `set_var`/`remove_var` were made `unsafe` for.
        // `API_KEY_ENV_LOCK` serializes every test in this module that
        // touches `TYPESAFE_API_KEY_VAR`, and no other code in this crate
        // touches that variable, so no other thread observes a torn value.
        unsafe {
            match key {
                Some(key) => std::env::set_var(TYPESAFE_API_KEY_VAR, key),
                None => std::env::remove_var(TYPESAFE_API_KEY_VAR),
            }
        }
        let result = body().await;
        // SAFETY: see above.
        unsafe {
            match previous {
                Some(value) => std::env::set_var(TYPESAFE_API_KEY_VAR, value),
                None => std::env::remove_var(TYPESAFE_API_KEY_VAR),
            }
        }
        result
    }

    #[tokio::test]
    async fn happy_path_noul() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {"is_urgent": {"type": "noul", "noul": 0.95}},
                "usage": {"input_tokens": 296, "output_tokens": 20},
            })))
            .mount(&server)
            .await;

        let client = JevClient::new(server.uri()).unwrap();
        let request = sample_request();
        let response = with_api_key(Some("test-key"), || client.ask(&request))
            .await
            .expect("happy path succeeds");

        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.usage.input_tokens, 296);
        // `answers` entries are raw `Value`s (see `SystemOneResponse::answers`
        // in `types.rs`); a caller parses the one it wants on demand.
        let answer: crate::checks::jev::types::Answer =
            serde_json::from_value(response.answers.get("is_urgent").unwrap().clone()).unwrap();
        match answer {
            crate::checks::jev::types::Answer::Noul(answer) => {
                assert_eq!(answer.noul, 0.95);
            }
            other => panic!("expected a Noul answer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn retries_429_then_succeeds() {
        let server = MockServer::start().await;
        // First response: 429. Second: success. Equal (default) priority
        // falls back to insertion order, so the 429 mock is tried first; once
        // its `up_to_n_times(1)` budget is spent it stops matching and the
        // second (unconstrained) mock takes over.
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(429))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "jev-1.13.0",
                "answers": {"is_urgent": {"type": "noul", "noul": 0.5}},
                "usage": {"input_tokens": 10},
            })))
            .mount(&server)
            .await;

        let client = JevClient::new(server.uri()).unwrap();
        let request = sample_request();
        let response = with_api_key(Some("test-key"), || client.ask(&request))
            .await
            .expect("retry then success");

        assert_eq!(response.model, "jev-1.13.0");
    }

    #[tokio::test]
    async fn exhausts_after_529_every_time() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(529))
            .mount(&server)
            .await;

        let client = JevClient::new(server.uri()).unwrap();
        let request = sample_request();
        let err = with_api_key(Some("test-key"), || client.ask(&request))
            .await
            .expect_err("every attempt is 529");

        match err {
            JevError::Exhausted { status, attempts } => {
                assert_eq!(status, 529);
                assert_eq!(attempts, MAX_ATTEMPTS);
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unauthorized_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let client = JevClient::new(server.uri()).unwrap();
        let request = sample_request();
        let err = with_api_key(Some("test-key"), || client.ask(&request))
            .await
            .expect_err("401 is unauthorized");
        assert!(matches!(err, JevError::Unauthorized));
    }

    #[tokio::test]
    async fn invalid_on_422_carries_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(
                ResponseTemplate::new(422).set_body_string("missing required field `questions`"),
            )
            .mount(&server)
            .await;

        let client = JevClient::new(server.uri()).unwrap();
        let request = sample_request();
        let err = with_api_key(Some("test-key"), || client.ask(&request))
            .await
            .expect_err("422 is invalid");
        match err {
            JevError::Invalid { body } => {
                assert!(body.contains("questions"), "got: {body}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_body_is_transport_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = JevClient::new(server.uri()).unwrap();
        let request = sample_request();
        let err = with_api_key(Some("test-key"), || client.ask(&request))
            .await
            .expect_err("malformed body fails");
        assert!(matches!(err, JevError::Transport(_)));
    }

    #[tokio::test]
    async fn unlisted_status_is_transport_with_status_and_truncated_body() {
        let server = MockServer::start().await;
        let huge_body = "x".repeat(MAX_TRANSPORT_BODY_BYTES * 2);
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(500).set_body_string(huge_body.clone()))
            .mount(&server)
            .await;

        let client = JevClient::new(server.uri()).unwrap();
        let request = sample_request();
        let err = with_api_key(Some("test-key"), || client.ask(&request))
            .await
            .expect_err("500 is not specially handled");
        match err {
            JevError::Transport(message) => {
                assert!(message.contains("500"), "got: {message}");
                assert!(
                    message.len() < huge_body.len(),
                    "body should have been truncated: {message}"
                );
                assert!(message.contains("truncated"), "got: {message}");
            }
            other => panic!("expected Transport, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn constructing_without_api_key_succeeds() {
        with_api_key(None, || async {
            JevClient::new("https://api.typesafe.ai").expect("construction never needs a key");
        })
        .await;
    }

    #[tokio::test]
    async fn first_request_without_api_key_fails_with_diagnostic() {
        // No mock server needed: the client must fail before sending anything.
        let client = JevClient::new("https://unused.invalid").unwrap();
        let request = sample_request();
        let err = with_api_key(None, || client.ask(&request))
            .await
            .expect_err("missing key fails the first request");
        assert!(matches!(err, JevError::MissingApiKey));
        assert!(err.to_string().contains(TYPESAFE_API_KEY_VAR));
    }
}
