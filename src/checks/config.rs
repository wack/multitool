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
/// executor; not yet mapped to a concrete `claude -p` flag for the MVP
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
    /// The model family to run (default: the `sonnet` family).
    pub model: String,
    /// The effort level (default: low).
    pub effort: Effort,
    /// Maximum number of checks executed concurrently.
    pub concurrency: usize,
    /// Per-agent wall-clock timeout (reaps an agent that hangs before reporting).
    pub agent_timeout: Duration,
    /// How many times to (re)run a check whose agent fails to report. Agents are
    /// nondeterministic and occasionally hang or finish without calling the
    /// report tool; a fresh attempt against the same endpoint usually succeeds.
    /// A check only resolves as errored after all attempts are exhausted.
    pub max_attempts: usize,
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
/// Hardcoded to `claude -p` + the `sonnet` family. Environment/file loading is
/// explicitly out of scope (see *Future work: global model configuration*).
pub fn configuration() -> Config {
    Config {
        provider_url: None,
        // The `sonnet` family. (The original MVP target was `haiku`, but on the
        // multi-file *reasoning* checks this feature exists for, haiku reliably
        // spins in a runaway exploration loop and never reaches a verdict;
        // sonnet reasons efficiently and reports in well under a minute.)
        model: "sonnet".to_string(),
        effort: Effort::Low,
        // Each check is a full `claude` agent process, killed the instant it
        // reports (see execution::run_one), so they don't linger. A small fan-out
        // gives each (CPU-heavy) reasoning agent enough cores to finish promptly.
        concurrency: 2,
        // Reaps an agent that hangs *before* reporting so the check can be
        // retried. Generous: the heaviest reasoning checks can take a few minutes
        // under contention before they report.
        agent_timeout: Duration::from_secs(240),
        max_attempts: 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_hardcoded_and_bounded_concurrency() {
        let cfg = configuration();
        assert_eq!(cfg.model, "sonnet");
        assert!(cfg.concurrency >= 1);
        // The executor is constructible (DI seam works).
        let _exec = cfg.build_executor();
    }
}
