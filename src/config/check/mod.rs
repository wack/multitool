use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use clap::Args;

use crate::checks::config::{CliOverrides, Effort, ProviderKind};

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

    /// Skip the freshness cache and always consult Jev, even when a check's
    /// frozen evidence replays byte-identical to its plan (jev builds only:
    /// this flag does not exist in a default-feature build's clap surface).
    #[cfg(feature = "jev")]
    #[arg(long)]
    no_cache: bool,

    /// Never write `.check-plan.toml` and never self-heal a stale entry —
    /// for CI, where the working tree must not be mutated (jev builds only:
    /// this flag does not exist in a default-feature build's clap surface).
    #[cfg(feature = "jev")]
    #[arg(long)]
    frozen: bool,
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
            self.concurrency.map(NonZeroUsize::get),
            self.trace_archive.clone(),
        )
    }

    /// Whether `--no-cache` was passed (jev builds only). Always `false` in
    /// a default-feature build, where the flag doesn't exist in the clap
    /// surface at all — this getter still exists there (rather than being
    /// `#[cfg(feature = "jev")]` itself) so `crate::checks::run`'s call site
    /// stays identical across both feature sets.
    #[cfg(feature = "jev")]
    pub fn no_cache(&self) -> bool {
        self.no_cache
    }

    /// See the `#[cfg(feature = "jev")]` overload's docs.
    #[cfg(not(feature = "jev"))]
    pub fn no_cache(&self) -> bool {
        false
    }

    /// Whether `--frozen` was passed (jev builds only, MULTI-1826). Always
    /// `false` in a default-feature build, where the flag doesn't exist in
    /// the clap surface at all — see [`Self::no_cache`]'s docs for why this
    /// getter still exists unconditionally rather than being itself
    /// `#[cfg(feature = "jev")]`.
    #[cfg(feature = "jev")]
    pub fn frozen(&self) -> bool {
        self.frozen
    }

    /// See the `#[cfg(feature = "jev")]` overload's docs.
    #[cfg(not(feature = "jev"))]
    pub fn frozen(&self) -> bool {
        false
    }
}
