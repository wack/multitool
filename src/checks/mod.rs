//! `multi check`: validate declared non-functional ("ility") requirements by
//! running AI-agent checks, reporting results through an in-process MCP server.
//!
//! The feature is a four-phase pipeline, each phase living in its own submodule:
//!
//! 1. [`config`] — the (hardcoded, dependency-injected) configuration phase.
//! 2. [`discovery`] — find/parse `CHECKS.md` files into a `Vec<Requirement>`.
//! 3. [`execution`] — run each check in a CoW [`sandbox`] via a boxed
//!    [`executor`], reporting verdicts through the [`mcp`] result server.
//! 4. [`reporting`] — render verdicts and produce the process exit code.

pub mod config;
mod discovery;
mod execution;
pub mod executor;
pub mod mcp;
pub mod model;
mod reporting;
pub mod sandbox;

#[cfg(test)]
mod e2e;

use std::path::Path;

use miette::Result;

use crate::Terminal;
use crate::checks::config::CliOverrides;

/// Run the full `multi check` pipeline rooted at `working_dir`.
///
/// Returns the process exit code: `0` if every requirement is satisfied (an
/// empty tree counts as success), `1` if any requirement is unsatisfied.
/// Operational errors (e.g. an invalid `CHECKS.md`) surface as `Err` diagnostics
/// rather than an exit code, so CI can tell "checks failed" from "tool errored".
pub async fn run(terminal: &Terminal, working_dir: &Path, overrides: CliOverrides) -> Result<i32> {
    // Phase 1: configuration — resolve provider/model/effort (flag > env > file)
    // and construct the provider registry, injected forward.
    let resolved = config::load(overrides)?;
    let cfg = resolved.config;

    // The provider registry is constructed and handed off here; wiring it into a
    // real executor is a follow-up. For now the MVP `ClaudeExecutor` runs the
    // checks, so just record what was built.
    tracing::debug!(
        provider = cfg.provider.as_str(),
        model = %cfg.model,
        available_providers = ?resolved.providers.keys().collect::<Vec<_>>(),
        "resolved checks configuration and provider registry",
    );

    // Phase 2: discovery.
    let requirements = discovery::discover(working_dir).await?;

    // Phase 3: execution — the executor is built from config and injected.
    let executor = cfg.build_executor();
    let outcomes = execution::execution_phase(&cfg, executor, working_dir, &requirements).await?;

    // Phase 4: reporting + exit code.
    reporting::report(terminal, &outcomes)
}
