//! The configuration phase (M2 #1341).
//!
//! For the MVP this is **hardcoded** (no env/file loading), but its values are
//! **dependency-injected** forward — execution receives a [`BoxedExecutor`] built
//! from this [`Config`] rather than reading provider details at point of use. So
//! swapping providers later is a config change, not a rewrite.

use std::time::Duration;

use crate::checks::executor::BoxedExecutor;
use crate::checks::executor::claude::ClaudeExecutor;

/// Effort level for the agent. Carried through configuration and logged by the
/// executor; not yet mapped to a concrete `claude -p` flag for the Haiku MVP
/// (see TODO in the executor). `Medium`/`High` are reserved for richer providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Medium/High are reserved for future provider wiring.
pub enum Effort {
    Low,
    Medium,
    High,
}

/// The resolved configuration for a `multi check` run.
#[derive(Debug, Clone)]
pub struct Config {
    /// Optional model-provider base URL. `None` uses the `claude` CLI default.
    pub provider_url: Option<String>,
    /// The model family to run (default: the `haiku` family).
    pub model: String,
    /// The effort level (default: low).
    pub effort: Effort,
    /// Maximum number of checks executed concurrently.
    pub concurrency: usize,
    /// Grace period to wait for a missing MCP report before failing a check.
    pub report_grace: Duration,
    /// Per-agent wall-clock timeout.
    pub agent_timeout: Duration,
}

impl Config {
    /// Construct the concrete [`BoxedExecutor`] from this configuration. This is
    /// the injection point: execution is handed the boxed executor, never a
    /// concrete type or a global.
    pub fn build_executor(&self) -> BoxedExecutor {
        Box::new(ClaudeExecutor::new(
            self.model.clone(),
            self.provider_url.clone(),
            self.effort,
            self.agent_timeout,
        ))
    }
}

/// The configuration phase: produce the hardcoded MVP [`Config`].
///
/// Hardcoded to `claude -p` + the `haiku` family. Environment/file loading is
/// explicitly out of scope (see *Future work: global model configuration*).
pub fn configuration() -> Config {
    Config {
        provider_url: None,
        model: "haiku".to_string(),
        effort: Effort::Low,
        concurrency: 8,
        report_grace: Duration::from_secs(10),
        agent_timeout: Duration::from_secs(300),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_haiku_and_bounded_concurrency() {
        let cfg = configuration();
        assert_eq!(cfg.model, "haiku");
        assert!(cfg.concurrency >= 1);
        // The executor is constructible (DI seam works).
        let _exec = cfg.build_executor();
    }
}
