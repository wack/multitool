use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use clap::Args;

use crate::checks::config::{CliOverrides, Effort, ExecutorKind, ProviderKind};

/// `multi check`: validate the requirements declared in `CHECKS.md` files.
///
/// The model/provider/effort flags are the highest-precedence config layer
/// (`flag > env > file`). They are intentionally `Option<T>` with **no**
/// `default_value`: an unset flag must contribute nothing to the figment merge,
/// otherwise clap's defaults would silently clobber the env/file layers.
#[derive(Args, Clone)]
pub struct CheckSubcommand {
    /// The directory to recursively scan for `CHECKS.md` files.
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

    /// The execution engine: `cersei` (in-process, default) or `claude` (the
    /// legacy `claude -p` fallback). Overrides `checks.executor` from env/file.
    #[arg(long, value_enum)]
    executor: Option<ExecutorKind>,

    /// Maximum number of checks to run concurrently. Must be greater than 0.
    /// Overrides `checks.concurrency` from env/file. Defaults to the number of
    /// available CPU cores.
    #[arg(long)]
    concurrency: Option<NonZeroUsize>,

    /// Capture every agent check session and bundle the traces into this
    /// `.tar.gz` archive (off by default). One directory per requirement, one
    /// file per check execution named by check title and numbered by retry.
    /// Overrides `checks.trace_archive` from env/file.
    #[arg(long, value_name = "PATH")]
    trace_archive: Option<PathBuf>,
}

impl CheckSubcommand {
    /// The directory to scan (defaults to the current directory).
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The flag layer for the config merge, carrying only the values the user
    /// actually passed.
    pub fn overrides(&self) -> CliOverrides {
        CliOverrides::new(
            self.provider,
            self.model.clone(),
            self.effort,
            self.executor,
            self.concurrency.map(NonZeroUsize::get),
            self.trace_archive.clone(),
        )
    }
}
