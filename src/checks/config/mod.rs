//! The configuration phase (M2 #1341, global config #1359, executor wiring #1367).
//!
//! Resolves the global default **provider**, **model**, and **effort** from
//! three sources with standard CLI precedence — `flag > env var > config file`
//! — merged with [`figment`], constructs a registry of ready-to-use model
//! providers, and builds the [`BoxedExecutor`] from a per-provider
//! [`ProviderFactory`].
//!
//! The resolved [`Config`] is **dependency-injected** forward: execution
//! receives a [`BoxedExecutor`] (see [`Resolved::build_executor`]) rather than
//! reading provider details at point of use. The executor is the in-process
//! [`CerseiExecutor`].

mod file;
mod jev;
mod models;
mod providers;
mod schema;

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use figment::{
    Figment,
    providers::{Env, Serialized},
};
use miette::{Result, miette};

use crate::checks::executor::BoxedExecutor;
use crate::checks::executor::cersei::CerseiExecutor;

// `JevConfig` is used unconditionally below, as `load_jev`'s return type —
// `load_jev` itself exists to serve `multi plan` (MULTI-1824,
// `--features jev`), but the function is defined (and this import used)
// regardless of that feature, so no `#[allow(unused_imports)]` is needed
// (contrast `resolve_jev`, just below, which stays un-re-exported since
// nothing outside this module calls it directly).
pub use jev::JevConfig;
pub use providers::{ProviderFactory, ProviderRegistry};
pub use schema::{CliOverrides, Effort, ProviderKind};

/// Per-agent wall-clock timeout. Generous: the heaviest reasoning checks can
/// take a few minutes under contention before they report.
const DEFAULT_AGENT_TIMEOUT: Duration = Duration::from_secs(240);
/// How many times to (re)run a check whose agent fails to report.
const DEFAULT_MAX_ATTEMPTS: usize = 3;

/// Default number of checks executed concurrently: one per available CPU core,
/// so a check suite fans out to use the whole machine rather than leaving cores
/// idle. Falls back to `1` on the rare platform where the count can't be
/// determined.
fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
}

/// The resolved configuration for a `multi check` run.
#[derive(Debug, Clone)]
pub struct Config {
    /// The selected provider.
    pub provider: ProviderKind,
    /// The selected provider's optional base-URL override, if configured
    /// (applied by the in-process executor via the provider factory); `None`
    /// uses the default.
    pub provider_url: Option<String>,
    /// The concrete model ID to run (validated against the hardcoded allowlist).
    pub model: String,
    /// The effort level.
    pub effort: Effort,
    /// Maximum number of checks executed concurrently (default: the number of
    /// available CPU cores; see [`default_concurrency`]).
    pub concurrency: usize,
    /// Per-agent wall-clock timeout (reaps an agent that hangs before reporting).
    pub agent_timeout: Duration,
    /// How many times to (re)run a check whose agent fails to report. Agents are
    /// nondeterministic and occasionally hit the turn cap or finish without
    /// calling the judge tool; a fresh attempt usually succeeds. A check only
    /// resolves as errored after all attempts are exhausted.
    pub max_attempts: usize,
    /// Where to bundle the opt-in session-trace archive, or `None` (default) to
    /// disable trace capture. See [`crate::checks::trace_archive`].
    pub trace_archive: Option<PathBuf>,
}

/// A single-variable env layer that maps `MULTI_CHECKS_JEV_BASE_URL` exactly
/// onto `checks.jev.base_url`.
///
/// The generic `Env::prefixed("MULTI_").split("_")` layer can't reach this key:
/// splitting on every `_` turns `CHECKS_JEV_BASE_URL` into the 4-level path
/// `checks.jev.base.url`, not the 3-level `checks.jev.base_url` our schema
/// actually has (`base_url` is one field, itself containing an underscore).
/// `checks.jev.base_url` is deliberately the *only* way to point Jev at a
/// different endpoint (there is no `TYPESAFE_BASE_URL`; see MULTI-1417), so an
/// env override of it is a required capability, not an edge case — this layer
/// exists specifically to provide it without touching the generic splitter (or
/// the pre-existing, equally-affected `[checks.providers.*].base_url`, which
/// is out of scope here).
///
/// Built from `Env::raw()` (no prefix stripping) rather than
/// `Env::prefixed("MULTI_")`, so `.only()` matches the *whole* env var name
/// exactly, then `.map()` rewrites that one match straight to the target key
/// path (figment nests on `.`, so the mapped key already encodes
/// `checks` → `jev` → `base_url`).
fn jev_base_url_env_layer() -> Env {
    Env::raw()
        .only(&["MULTI_CHECKS_JEV_BASE_URL"])
        .map(|_| "checks.jev.base_url".into())
}

/// Merge the three config layers and extract the resolved `[checks]` table.
///
/// Merge order (low → high) is `file → MULTI_-prefixed env → the targeted
/// `checks.jev.base_url` env layer → flags`, giving `flag > env > file`.
/// figment's own CLI example orders env-highest; we invert to flag-highest.
/// The flag layer only serialises values the user actually passed (the CLI
/// fields are `Option<T>` with no clap default), so an unset flag contributes
/// nothing and does not clobber env/file.
fn resolve_layers(
    file_layer: schema::RootFileConfig,
    overrides: CliOverrides,
) -> Result<schema::ChecksSection> {
    let figment = Figment::new()
        .merge(Serialized::defaults(file_layer))
        .merge(Env::prefixed("MULTI_").split("_"))
        .merge(jev_base_url_env_layer())
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
    /// Construct the raw in-process reasoning-agent executor
    /// ([`CerseiExecutor`]), with **no** Jev wrapping regardless of feature.
    ///
    /// `multi plan`'s `AgentPlanner` (MULTI-1824, `--features jev`) calls
    /// this directly rather than [`Self::build_executor`]: it exists
    /// specifically to run the reasoning agent so it can (re-)establish a
    /// plan entry, and wrapping it in
    /// `JevExecutor`([`crate::checks::jev::executor::JevExecutor`]) would let
    /// a cached/Jev-decided verdict silently short-circuit that very run —
    /// `entry_from_outcome` would then see an outcome with zero captured
    /// tool calls (a well-formed `Cached`/`Jev` `AgentOutcome` never ran an
    /// agent) and overwrite the entry being (re-)planned with a degenerate
    /// "no tool calls" one. [`Self::build_executor`] (the switch `multi
    /// check`'s pipeline goes through) composes this same executor as its
    /// own inner, agent-escalation target.
    pub fn build_agent_executor(&self) -> CerseiExecutor {
        let cfg = &self.config;
        CerseiExecutor::new(
            self.factory.clone(),
            cfg.model.clone(),
            cfg.effort,
            cfg.agent_timeout,
            // The archive path lives at the orchestration layer; the executor
            // only needs to know whether to capture a per-execution trace.
            cfg.trace_archive.is_some(),
        )
    }

    /// Construct the [`BoxedExecutor`] `multi check`'s pipeline runs — the
    /// single `cfg` switch (MULTI-1825): [`Self::build_agent_executor`]
    /// alone in the default build.
    #[cfg(not(feature = "jev"))]
    pub fn build_executor(&self) -> Result<BoxedExecutor> {
        Ok(Box::new(self.build_agent_executor()))
    }

    /// See the `#[cfg(not(feature = "jev"))]` overload's docs: under
    /// `--features jev`, [`Self::build_agent_executor`] wrapped in
    /// [`crate::checks::jev::executor::JevExecutor`] for escalation.
    /// `jev_client`/`jev_config` are resolved separately from `Self` — see
    /// [`load_jev`], whose own docs explain why `multi check`'s [`load`]
    /// deliberately never validates `[checks.jev]` — `no_cache` is `multi
    /// check --no-cache`, and `frozen` is `multi check --frozen`
    /// (MULTI-1826).
    #[cfg(feature = "jev")]
    pub fn build_executor(
        &self,
        jev_client: std::sync::Arc<crate::checks::jev::client::JevClient>,
        jev_config: JevConfig,
        no_cache: bool,
        frozen: bool,
    ) -> Result<BoxedExecutor> {
        let inner: std::sync::Arc<dyn crate::checks::executor::CheckExecutor + Send + Sync> =
            std::sync::Arc::new(self.build_agent_executor());
        Ok(Box::new(crate::checks::jev::executor::JevExecutor::new(
            inner, jev_client, jev_config, no_cache, frozen,
        )))
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
    let concurrency = checks.concurrency.unwrap_or_else(default_concurrency);

    if !models::is_valid_model(provider, &model) {
        return Err(miette!(
            "model `{model}` is not a valid model for provider `{}`. Valid models: {}",
            provider.as_str(),
            models::models_for(provider).join(", "),
        ));
    }

    if concurrency == 0 {
        return Err(miette!("checks.concurrency must be greater than 0"));
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
        concurrency,
        agent_timeout: DEFAULT_AGENT_TIMEOUT,
        max_attempts: DEFAULT_MAX_ATTEMPTS,
        trace_archive: checks.trace_archive,
    };

    Ok(Resolved {
        config,
        providers: registry,
        factory,
    })
}

/// Resolve `[checks.jev]` alone from the standard `flag > env > file` merge —
/// the same layers [`load`] merges, without touching [`Config`]/[`Resolved`]
/// at all. Used only by `multi plan` (MULTI-1824, `--features jev`):
/// `multi check`'s [`load`]/[`Resolved`] deliberately do **not** carry this.
/// Folding it into `load` instead (so both commands shared one merge) would
/// have made `multi check` start validating `checks.jev.threshold` in every
/// build — a malformed `[checks.jev]` table was never checked on that path
/// before this ticket (see [`jev::resolve_jev`]'s own doc comment: it "has no
/// caller outside this module's own tests in *either* build yet"), and
/// starting to reject one now would be a default-build behavior change this
/// ticket doesn't call for. The one cost is that `multi plan` merges the
/// config file/env twice (once here, once via its own `load` call) — cheap,
/// and not a duplicated *validation* concern, since the two merges validate
/// disjoint fields.
pub fn load_jev(overrides: CliOverrides) -> Result<JevConfig> {
    let checks = resolve_layers(file::load_file_layer(), overrides)?;
    jev::resolve_jev(&checks.jev)
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
        concurrency: default_concurrency(),
        agent_timeout: DEFAULT_AGENT_TIMEOUT,
        max_attempts: DEFAULT_MAX_ATTEMPTS,
        trace_archive: None,
    }
}

#[cfg(test)]
mod tests {
    // `figment::Jail`'s closure returns `Result<(), figment::Error>`, and that
    // error is large — unavoidable given the API, so allow it in these tests.
    #![allow(clippy::result_large_err)]

    use super::*;
    use figment::Jail;
    use schema::{
        ChecksSection, CliChecksOverrides, CliJevOverrides, JevSection, ProviderOverrides,
        ProvidersSection, RootFileConfig,
    };

    fn file_with(provider: ProviderKind, model: &str) -> RootFileConfig {
        RootFileConfig {
            checks: ChecksSection {
                provider: Some(provider),
                model: Some(model.to_string()),
                effort: Some(Effort::Low),
                concurrency: None,
                trace_archive: None,
                providers: ProvidersSection::default(),
                jev: JevSection::default(),
            },
        }
    }

    #[test]
    fn defaults_are_hardcoded_and_bounded_concurrency() {
        let cfg = configuration();
        assert_eq!(cfg.provider, ProviderKind::Anthropic);
        assert_eq!(cfg.model, "claude-sonnet-4-6");
        assert!(cfg.concurrency >= 1);
        // The default must track the machine's core count, not a hardcoded value.
        assert_eq!(cfg.concurrency, default_concurrency());
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
            let overrides =
                CliOverrides::new(None, Some("claude-opus-4-8".into()), None, None, None);
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

    #[test]
    fn jev_table_is_read_from_file() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "MultiTool.toml",
                r#"
[checks.jev]
model = "jev-preview"
threshold = 0.9
base_url = "https://jev.example"
"#,
            )?;
            let file = file::load_file_layer();
            let checks = resolve_layers(file, CliOverrides::default()).unwrap();
            assert_eq!(checks.jev.model.as_deref(), Some("jev-preview"));
            assert_eq!(checks.jev.threshold, Some(0.9));
            assert_eq!(checks.jev.base_url.as_deref(), Some("https://jev.example"));
            Ok(())
        });
    }

    #[test]
    fn jev_env_beats_file() {
        Jail::expect_with(|jail| {
            // `MULTI_CHECKS_JEV_MODEL`/`MULTI_CHECKS_JEV_THRESHOLD` map cleanly
            // onto `checks.jev.{model,threshold}` via the `_` split: each is a
            // single-word field, so the split produces exactly the right nesting.
            jail.set_env("MULTI_CHECKS_JEV_MODEL", "jev-preview");
            jail.set_env("MULTI_CHECKS_JEV_THRESHOLD", "0.5");

            let mut file = RootFileConfig::default();
            file.checks.jev.model = Some("jev-latest".to_string());
            file.checks.jev.threshold = Some(0.75);

            let checks = resolve_layers(file, CliOverrides::default()).unwrap();
            assert_eq!(checks.jev.model.as_deref(), Some("jev-preview"));
            assert_eq!(checks.jev.threshold, Some(0.5));
            Ok(())
        });
    }

    #[test]
    fn jev_flag_beats_env_and_file() {
        Jail::expect_with(|jail| {
            jail.set_env("MULTI_CHECKS_JEV_MODEL", "jev-preview");

            let mut file = RootFileConfig::default();
            file.checks.jev.model = Some("jev-latest".to_string());

            let overrides = CliOverrides {
                checks: CliChecksOverrides {
                    jev: CliJevOverrides {
                        model: Some("jev-flag-override".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            };

            let checks = resolve_layers(file, overrides).unwrap();
            assert_eq!(checks.jev.model.as_deref(), Some("jev-flag-override"));
            Ok(())
        });
    }

    #[test]
    fn jev_base_url_env_var_sets_base_url() {
        // `MULTI_CHECKS_JEV_BASE_URL` reaches `checks.jev.base_url` via the
        // targeted `jev_base_url_env_layer`, not the generic `_`-splitting
        // layer (which can't: see that function's doc comment).
        Jail::expect_with(|jail| {
            jail.set_env("MULTI_CHECKS_JEV_BASE_URL", "https://jev.example");
            let checks =
                resolve_layers(RootFileConfig::default(), CliOverrides::default()).unwrap();
            assert_eq!(checks.jev.base_url.as_deref(), Some("https://jev.example"));
            Ok(())
        });
    }

    #[test]
    fn jev_base_url_env_beats_file() {
        Jail::expect_with(|jail| {
            jail.set_env("MULTI_CHECKS_JEV_BASE_URL", "https://env.example");

            let mut file = RootFileConfig::default();
            file.checks.jev.base_url = Some("https://file.example".to_string());

            let checks = resolve_layers(file, CliOverrides::default()).unwrap();
            assert_eq!(checks.jev.base_url.as_deref(), Some("https://env.example"));
            Ok(())
        });
    }

    #[test]
    fn jev_base_url_flag_beats_env() {
        Jail::expect_with(|jail| {
            jail.set_env("MULTI_CHECKS_JEV_BASE_URL", "https://env.example");

            let overrides = CliOverrides {
                checks: CliChecksOverrides {
                    jev: CliJevOverrides {
                        base_url: Some("https://flag.example".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            };

            let checks = resolve_layers(RootFileConfig::default(), overrides).unwrap();
            assert_eq!(checks.jev.base_url.as_deref(), Some("https://flag.example"));
            Ok(())
        });
    }

    #[test]
    fn jev_base_url_env_var_coexists_with_the_generic_splitter_stray_key() {
        // The generic `Env::prefixed("MULTI_").split("_")` layer *also* reads
        // `MULTI_CHECKS_JEV_BASE_URL`, but maps it to the unrelated key path
        // `checks.jev.base.url` (see `jev_base_url_env_layer`'s doc comment).
        // `JevSection` has no `#[serde(deny_unknown_fields)]` (matching every
        // other config struct in this module — see e.g. `RootFileConfig`'s own
        // doc comment), so that stray `base` key is silently ignored rather
        // than breaking deserialization of the real `base_url` the targeted
        // layer sets.
        Jail::expect_with(|jail| {
            jail.set_env("MULTI_CHECKS_JEV_BASE_URL", "https://jev.example");
            let checks =
                resolve_layers(RootFileConfig::default(), CliOverrides::default()).unwrap();
            assert_eq!(checks.jev.base_url.as_deref(), Some("https://jev.example"));
            assert_eq!(checks.jev.model, None);
            assert_eq!(checks.jev.threshold, None);
            Ok(())
        });
    }

    #[test]
    fn jev_config_resolves_from_merged_layers() {
        Jail::expect_with(|jail| {
            jail.create_file(
                "MultiTool.toml",
                r#"
[checks.jev]
threshold = 0.6
"#,
            )?;
            let file = file::load_file_layer();
            let checks = resolve_layers(file, CliOverrides::default()).unwrap();
            let jev_config = jev::resolve_jev(&checks.jev).expect("valid threshold");
            assert_eq!(jev_config.model, jev::DEFAULT_MODEL);
            assert_eq!(jev_config.threshold, 0.6);
            assert_eq!(jev_config.base_url, jev::DEFAULT_BASE_URL);
            Ok(())
        });
    }
}
