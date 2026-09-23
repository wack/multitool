use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use clap::Args;

use crate::checks::config::{CliOverrides, Effort, ProviderKind};

/// `multi plan`: establish what evidence is necessary to verify each check and
/// freeze it into `.check-plan.toml` (the Jev decision engine, MULTI-1824).
///
/// Mirrors [`crate::config::CheckSubcommand`]'s provider/model/effort/
/// concurrency flags exactly — same highest-precedence `flag > env > file`
/// semantics, same reason they're `Option<T>` with no `default_value` (see
/// that type's docs) — and adds `--force` to bypass the freshness cache.
/// There is no `--trace-archive`: planning runs the same reasoning agent as
/// `multi check`, but trace capture is out of scope for this milestone.
#[derive(Args, Clone)]
pub struct PlanSubcommand {
    /// The directory to recursively scan for `CHECKS.toml` files.
    #[arg(default_value = ".")]
    directory: PathBuf,

    /// The model provider to use. Overrides `checks.provider` from env/file.
    #[arg(long, value_enum)]
    provider: Option<ProviderKind>,

    /// The concrete model ID to run. Overrides `checks.model` from env/file.
    #[arg(long)]
    model: Option<String>,

    /// The agent effort level. Overrides `checks.effort` from env/file.
    #[arg(long, value_enum)]
    effort: Option<Effort>,

    /// Maximum number of checks planned concurrently. Must be greater than 0.
    /// Overrides `checks.concurrency` from env/file. Defaults to the number of
    /// available CPU cores.
    #[arg(long)]
    concurrency: Option<NonZeroUsize>,

    /// Re-plan every check from scratch, bypassing the freshness cache (which
    /// otherwise reuses an existing `.check-plan.toml` entry — zero agent runs,
    /// zero Jev calls — whenever its frozen evidence replays fresh).
    #[arg(long)]
    force: bool,

    /// Run each check's agent in a copy-on-write clone of its repository root
    /// (off by default). Planning agents only get read-only tools, so without
    /// this they read the repository root directly.
    #[arg(long)]
    sandbox: bool,
}

impl PlanSubcommand {
    /// The directory to scan (defaults to the current directory).
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The flag layer for the config merge, carrying only the values the user
    /// actually passed. `multi plan` has no `--trace-archive` flag, so that
    /// field is always unset here.
    pub fn overrides(&self) -> CliOverrides {
        CliOverrides::new(
            self.provider,
            self.model.clone(),
            self.effort,
            self.concurrency.map(NonZeroUsize::get),
            None,
        )
    }

    /// Whether `--force` was passed: bypass the freshness cache and re-plan
    /// every check from scratch.
    pub fn force(&self) -> bool {
        self.force
    }

    /// Whether `--sandbox` was passed: run each agent in a CoW clone.
    pub fn sandbox(&self) -> bool {
        self.sandbox
    }
}
