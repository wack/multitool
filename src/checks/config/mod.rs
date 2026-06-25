//! The configuration phase (M2 #1341, global config #1359, executor wiring #1367).
//!
//! Resolves the global default **provider**, **model**, **effort**, and
//! **executor** from three sources with standard CLI precedence —
//! `flag > env var > config file` — merged with [`figment`], constructs a
//! registry of ready-to-use model providers, and builds the selected
//! [`BoxedExecutor`] from a per-provider [`ProviderFactory`].
//!
//! The resolved [`Config`] is **dependency-injected** forward: execution
//! receives a [`BoxedExecutor`] (see [`Resolved::build_executor`]) rather than
//! reading provider details at point of use. The default executor is the
//! in-process [`CerseiExecutor`]; the legacy [`ClaudeExecutor`] remains
//! selectable as a migration fallback.

mod file;
mod models;
mod providers;
mod schema;

use std::time::Duration;

use figment::{
    Figment,
    providers::{Env, Serialized},
};
use miette::{Result, miette};

use crate::checks::executor::BoxedExecutor;
use crate::checks::executor::cersei::CerseiExecutor;
use crate::checks::executor::claude::ClaudeExecutor;

pub use providers::{ProviderFactory, ProviderRegistry};
pub use schema::{CliOverrides, Effort, ExecutorKind, ProviderKind};

/// Maximum number of checks executed concurrently. A small fan-out gives each
/// (CPU-heavy) reasoning agent enough cores to finish promptly.
const DEFAULT_CONCURRENCY: usize = 2;
/// Per-agent wall-clock timeout. Generous: the heaviest reasoning checks can
/// take a few minutes under contention before they report.
const DEFAULT_AGENT_TIMEOUT: Duration = Duration::from_secs(240);
/// How many times to (re)run a check whose agent fails to report.
const DEFAULT_MAX_ATTEMPTS: usize = 3;

/// The resolved configuration for a `multi check` run.
#[derive(Debug, Clone)]
pub struct Config {
    /// The selected provider.
    pub provider: ProviderKind,
    /// The selected provider's optional base-URL override, if configured. Used
    /// by the `claude -p` fallback as `ANTHROPIC_BASE_URL` (the in-process
    /// executor applies it via the provider factory); `None` uses the default.
    pub provider_url: Option<String>,
    /// The concrete model ID to run (validated against the hardcoded allowlist).
    pub model: String,
    /// The effort level.
    pub effort: Effort,
    /// Which execution engine runs each check (default: in-process cersei).
    pub executor: ExecutorKind,
    /// Maximum number of checks executed concurrently.
    pub concurrency: usize,
    /// Per-agent wall-clock timeout (reaps an agent that hangs before reporting).
    pub agent_timeout: Duration,
    /// How many times to (re)run a check whose agent fails to report. Agents are
    /// nondeterministic and occasionally hit the turn cap or finish without
    /// calling the judge tool; a fresh attempt usually succeeds. A check only
    /// resolves as errored after all attempts are exhausted.
    pub max_attempts: usize,
}

impl Config {
    /// Construct the legacy `claude -p` fallback executor from this
    /// configuration. The in-process cersei executor needs the resolved provider
    /// factory and so is built from [`Resolved`]; this builder only covers the
    /// fallback, which needs nothing beyond [`Config`].
    pub fn build_claude_executor(&self) -> BoxedExecutor {
        Box::new(ClaudeExecutor::new(
            self.model.clone(),
            self.provider_url.clone(),
            self.effort,
            self.agent_timeout,
        ))
    }
}

/// Merge the three config layers and extract the resolved `[checks]` table.
///
/// Merge order (low → high) is `file → MULTI_-prefixed env → flags`, giving
/// `flag > env > file`. figment's own CLI example orders env-highest; we invert
/// to flag-highest. The flag layer only serialises values the user actually
/// passed (the CLI fields are `Option<T>` with no clap default), so an unset
/// flag contributes nothing and does not clobber env/file.
fn resolve_layers(
    file_layer: schema::RootFileConfig,
    overrides: CliOverrides,
) -> Result<schema::ChecksSection> {
    let figment = Figment::new()
        .merge(Serialized::defaults(file_layer))
        .merge(Env::prefixed("MULTI_").split("_"))
        .merge(Serialized::defaults(overrides));

    let root: schema::RootFileConfig = figment
        .extract()
        .map_err(|e| miette!("invalid checks configuration: {e}"))?;
    Ok(root.checks)
}

/// The product of the configuration phase: the resolved [`Config`] the pipeline
/// consumes, the constructed [`ProviderRegistry`] (one live handle per available
/// provider), and the [`ProviderFactory`] for the *selected* provider that the
/// in-process executor uses to mint a fresh handle per check.
pub struct Resolved {
    pub config: Config,
    pub providers: ProviderRegistry,
    pub factory: ProviderFactory,
}

impl Resolved {
    /// Construct the selected [`BoxedExecutor`]. This is the injection point and
    /// the migration lever: `cersei` (default) runs the in-process agent;
    /// `claude` runs the legacy `claude -p` fallback over the same checks.
    pub fn build_executor(&self) -> Result<BoxedExecutor> {
        let cfg = &self.config;
        let executor: BoxedExecutor = match cfg.executor {
            ExecutorKind::Cersei => Box::new(CerseiExecutor::new(
                self.factory.clone(),
                cfg.model.clone(),
                cfg.effort,
                cfg.agent_timeout,
            )),
            ExecutorKind::Claude => cfg.build_claude_executor(),
        };
        Ok(executor)
    }
}

/// The configuration phase: resolve provider/model/effort from file + env +
/// flags, validate the model, and build the provider registry.
///
/// Merge order (low → high) is `file → MULTI_-prefixed env → flags`, giving
/// `flag > env > file`. Credentials are **not** part of this merge: API keys are
/// read straight from each provider's native env var at construction.
pub fn load(overrides: CliOverrides) -> Result<Resolved> {
    let checks = resolve_layers(file::load_file_layer(), overrides)?;

    let provider = checks.provider.unwrap_or(ProviderKind::Anthropic);
    let model = checks
        .model
        .unwrap_or_else(|| models::default_model(provider).to_string());
    let effort = checks.effort.unwrap_or(Effort::Low);
    let executor = checks.executor.unwrap_or(ExecutorKind::Cersei);

    if !models::is_valid_model(provider, &model) {
        return Err(miette!(
            "model `{model}` is not a valid model for provider `{}`. Valid models: {}",
            provider.as_str(),
            models::models_for(provider).join(", "),
        ));
    }

    // Build one handle per provider whose credential is present, then require
    // that the *selected* provider actually resolved to an available handle.
    let registry = providers::build_registry(&checks.providers)?;
    if !registry.contains_key(provider.as_str()) {
        return Err(miette!(
            "configured provider `{}` is unavailable: set its API key ({}) in the environment",
            provider.as_str(),
            providers::credential_env_hint(provider),
        ));
    }

    // The factory for the selected provider mints a fresh handle per check; its
    // credential is guaranteed present by the availability check above.
    let factory = providers::build_factory(provider, &checks.providers)
        .expect("selected provider availability was just verified");

    let provider_url = checks.providers.base_url(provider).map(ToOwned::to_owned);

    let config = Config {
        provider,
        provider_url,
        model,
        effort,
        executor,
        concurrency: DEFAULT_CONCURRENCY,
        agent_timeout: DEFAULT_AGENT_TIMEOUT,
        max_attempts: DEFAULT_MAX_ATTEMPTS,
    };

    Ok(Resolved {
        config,
        providers: registry,
        factory,
    })
}

/// The hardcoded default [`Config`], with no file/env/flag loading. Used as the
/// baseline by tests and internal callers that don't need the resolved layer.
pub fn configuration() -> Config {
    let provider = ProviderKind::Anthropic;
    Config {
        provider,
        provider_url: None,
        // The `sonnet` family. (The original MVP target was `haiku`, but on the
        // multi-file *reasoning* checks this feature exists for, haiku reliably
        // spins in a runaway exploration loop and never reaches a verdict;
        // sonnet reasons efficiently and reports in well under a minute.)
        model: models::default_model(provider).to_string(),
        effort: Effort::Low,
        executor: ExecutorKind::Cersei,
        concurrency: DEFAULT_CONCURRENCY,
        agent_timeout: DEFAULT_AGENT_TIMEOUT,
        max_attempts: DEFAULT_MAX_ATTEMPTS,
    }
}

#[cfg(test)]
mod tests {
    // `figment::Jail`'s closure returns `Result<(), figment::Error>`, and that
    // error is large — unavoidable given the API, so allow it in these tests.
    #![allow(clippy::result_large_err)]

    use super::*;
    use figment::Jail;
    use schema::{ChecksSection, ProviderOverrides, ProvidersSection, RootFileConfig};

    fn file_with(provider: ProviderKind, model: &str) -> RootFileConfig {
        RootFileConfig {
            checks: ChecksSection {
                provider: Some(provider),
                model: Some(model.to_string()),
                effort: Some(Effort::Low),
                executor: None,
                providers: ProvidersSection::default(),
            },
        }
    }

    #[test]
    fn defaults_are_hardcoded_and_bounded_concurrency() {
        let cfg = configuration();
        assert_eq!(cfg.provider, ProviderKind::Anthropic);
        assert_eq!(cfg.model, "claude-sonnet-4-6");
        assert_eq!(cfg.executor, ExecutorKind::Cersei);
        assert!(cfg.concurrency >= 1);
        // The fallback executor is constructible from config alone (DI seam works).
        let _exec = cfg.build_claude_executor();
    }

    #[test]
    fn flag_beats_file() {
        Jail::expect_with(|_jail| {
            let file = file_with(ProviderKind::Anthropic, "claude-haiku-4-5");
            let overrides = CliOverrides::new(
                Some(ProviderKind::OpenAi),
                Some("gpt-4o".into()),
                None,
                None,
            );
            let checks = resolve_layers(file, overrides).unwrap();
            assert_eq!(checks.provider, Some(ProviderKind::OpenAi));
            assert_eq!(checks.model.as_deref(), Some("gpt-4o"));
            // `effort` was unset on the flag layer, so the file value survives.
            assert_eq!(checks.effort, Some(Effort::Low));
            Ok(())
        });
    }

    #[test]
    fn unset_flag_does_not_clobber_file() {
        Jail::expect_with(|_jail| {
            let file = file_with(ProviderKind::Anthropic, "claude-haiku-4-5");
            let checks = resolve_layers(file, CliOverrides::default()).unwrap();
            assert_eq!(checks.provider, Some(ProviderKind::Anthropic));
            assert_eq!(checks.model.as_deref(), Some("claude-haiku-4-5"));
            Ok(())
        });
    }

    #[test]
    fn env_beats_file_and_flag_beats_env() {
        Jail::expect_with(|jail| {
            // `MULTI_CHECKS_MODEL` maps to `checks.model` via the `_` split.
            jail.set_env("MULTI_CHECKS_MODEL", "claude-haiku-4-5");
            let file = file_with(ProviderKind::Anthropic, "claude-sonnet-4-6");

            // Env outranks the file...
            let checks = resolve_layers(file.clone(), CliOverrides::default()).unwrap();
            assert_eq!(checks.model.as_deref(), Some("claude-haiku-4-5"));

            // ...and a flag outranks env.
            let overrides = CliOverrides::new(None, Some("claude-opus-4-8".into()), None, None);
            let checks = resolve_layers(file, overrides).unwrap();
            assert_eq!(checks.model.as_deref(), Some("claude-opus-4-8"));
            Ok(())
        });
    }

    #[test]
    fn env_maps_provider_into_checks_namespace() {
        Jail::expect_with(|jail| {
            jail.set_env("MULTI_CHECKS_PROVIDER", "gemini");
            let checks =
                resolve_layers(RootFileConfig::default(), CliOverrides::default()).unwrap();
            assert_eq!(checks.provider, Some(ProviderKind::Gemini));
            Ok(())
        });
    }

    #[test]
    fn load_rejects_invalid_model() {
        Jail::expect_with(|jail| {
            // A syntactically fine but disallowed model ID. `load` validates
            // against the hardcoded allowlist *before* touching the provider
            // registry, so this fails deterministically without any API keys.
            jail.create_file(
                "MultiTool.toml",
                r#"
[checks]
provider = "anthropic"
model = "claude-totally-made-up"
"#,
            )?;
            let err = load(CliOverrides::default())
                .err()
                .expect("an invalid model must be rejected")
                .to_string();
            assert!(err.contains("claude-totally-made-up"), "got: {err}");
            Ok(())
        });
    }

    #[test]
    fn base_url_override_is_read_from_file() {
        let providers = ProvidersSection {
            anthropic: Some(ProviderOverrides {
                base_url: Some("https://example.test".into()),
            }),
            ..Default::default()
        };
        assert_eq!(
            providers.base_url(ProviderKind::Anthropic),
            Some("https://example.test"),
        );
        assert_eq!(providers.base_url(ProviderKind::OpenAi), None);
    }
}
