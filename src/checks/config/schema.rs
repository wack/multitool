//! The serde/clap schema shared by the three config sources.
//!
//! The same nested shape (`{ checks: { provider, model, effort, providers } }`)
//! is produced by the file loader, figment's `Env` provider, and the CLI
//! overrides, so figment can merge them by key path with `flag > env > file`
//! precedence. See [`super::load`].

use std::path::PathBuf;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// A model provider. Serialises to its lowercase name (`anthropic`, `openai`,
/// `gemini`) in every source — TOML, `MULTI_CHECKS_PROVIDER`, and `--provider`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    #[value(name = "openai")]
    OpenAi,
    Gemini,
}

impl ProviderKind {
    /// The canonical lowercase provider name, used as the registry key and as
    /// the `[checks.providers.<name>]` table name.
    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Gemini => "gemini",
        }
    }
}

/// The agent effort level. Carried through configuration and consumed by the
/// executor, where it maps to a thinking-token budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

/// The whole config file, of which only the `[checks]` table concerns us. Other
/// top-level keys (the legacy manifest's `workspace`/`application`/`config`) are
/// ignored rather than rejected, so a single `MultiTool.toml` can carry both.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RootFileConfig {
    #[serde(default)]
    pub checks: ChecksSection,
}

/// The `[checks]` table. Every selectable field is optional so an unset value in
/// a higher-precedence layer contributes nothing to the merge.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChecksSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// Maximum number of checks executed concurrently (default: the number of
    /// available CPU cores).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
    /// Where to write the opt-in session-trace archive. When set, every check
    /// execution's agent session is captured and bundled into this `.tar.gz`
    /// (see [`crate::checks::trace_archive`]); unset (the default) disables
    /// capture entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_archive: Option<PathBuf>,
    /// Optional, non-secret per-provider base-URL overrides.
    #[serde(default)]
    pub providers: ProvidersSection,
    /// The `[checks.jev]` table (TypeSafe/Jev decision-engine settings).
    /// Present unconditionally — not `#[cfg(feature = "jev")]` — so a config
    /// file carrying a `[checks.jev]` table parses identically whether or not
    /// the `jev` Cargo feature is compiled in; only `crate::checks::jev` (the
    /// feature-gated HTTP client) acts on the resolved values. See
    /// `super::jev::resolve_jev`.
    #[serde(default)]
    pub jev: JevSection,
}

/// `[checks.providers]` — at most one table per provider, each carrying an
/// optional `base_url`. Credentials never live here (env-only).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProvidersSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic: Option<ProviderOverrides>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openai: Option<ProviderOverrides>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ProviderOverrides>,
}

impl ProvidersSection {
    /// The configured base-URL override for `provider`, if any.
    pub fn base_url(&self, provider: ProviderKind) -> Option<&str> {
        let table = match provider {
            ProviderKind::Anthropic => &self.anthropic,
            ProviderKind::OpenAi => &self.openai,
            ProviderKind::Gemini => &self.gemini,
        };
        table.as_ref().and_then(|t| t.base_url.as_deref())
    }
}

/// The non-secret overrides for one provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// `[checks.jev]` — TypeSafe/Jev decision-engine settings. Every field is
/// optional so an unset value in a higher-precedence layer contributes nothing
/// to the merge (same convention as [`ChecksSection`]). Credentials never live
/// here: `TYPESAFE_API_KEY` is read directly from the environment (lazily, by
/// the feature-gated HTTP client), never from this table and never under
/// `MULTI_`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JevSection {
    /// The Jev model ID or alias (default: [`super::jev::DEFAULT_MODEL`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The confidence threshold gating a Jev-decided verdict, validated to
    /// `(0, 1]` (default: [`super::jev::DEFAULT_THRESHOLD`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// The TypeSafe SystemOne API origin. This is the **single** source of
    /// truth for the endpoint — there is no separate `TYPESAFE_BASE_URL`
    /// variable (avoids the split-source-of-truth problem in MULTI-1417).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

/// The flag layer, fed into figment via `Serialized::defaults`. Only the values
/// the user actually passed are serialised (`skip_serializing_if`), so unset
/// flags don't clobber the env/file layers — the clap-defaults gotcha the
/// ticket calls out. This is why the corresponding CLI fields carry no
/// `default_value`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CliOverrides {
    pub checks: CliChecksOverrides,
}

/// The `[checks]` subset settable from flags.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CliChecksOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_archive: Option<PathBuf>,
    /// The `[checks.jev]` subset settable from flags. `multi check` exposes no
    /// `--jev-*` flags yet (no command consumes Jev config until MULTI-1825),
    /// so this is always empty in practice today; it exists so the merge
    /// pipeline already supports a flag layer for `checks.jev.*` without
    /// reshaping [`CliOverrides`] when that flag surface is added.
    #[serde(default)]
    pub jev: CliJevOverrides,
}

/// The `[checks.jev]` subset settable from flags. See [`CliChecksOverrides::jev`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CliJevOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl CliOverrides {
    /// Build the flag layer from the individual CLI values.
    pub fn new(
        provider: Option<ProviderKind>,
        model: Option<String>,
        effort: Option<Effort>,
        concurrency: Option<usize>,
        trace_archive: Option<PathBuf>,
    ) -> Self {
        Self {
            checks: CliChecksOverrides {
                provider,
                model,
                effort,
                concurrency,
                trace_archive,
                jev: CliJevOverrides::default(),
            },
        }
    }
}
