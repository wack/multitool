//! The serde/clap schema shared by the three config sources.
//!
//! The same nested shape (`{ checks: { provider, model, effort, providers } }`)
//! is produced by the file loader, figment's `Env` provider, and the CLI
//! overrides, so figment can merge them by key path with `flag > env > file`
//! precedence. See [`super::load`].

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

/// Which execution engine runs each check. The default is the in-process
/// [`cersei`](crate::checks::executor::cersei) agent; `claude` selects the
/// legacy `claude -p` shell-out fallback, kept selectable during the migration
/// (MULTI-1367) so verdicts from both can be compared before the fallback is
/// retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ExecutorKind {
    /// The in-process `cersei-agent` executor (default).
    Cersei,
    /// The legacy `claude -p` shell-out fallback.
    Claude,
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
    /// Which execution engine runs each check (`cersei` by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<ExecutorKind>,
    /// Maximum number of checks executed concurrently (default: the number of
    /// available CPU cores).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
    /// Optional, non-secret per-provider base-URL overrides.
    #[serde(default)]
    pub providers: ProvidersSection,
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
    pub executor: Option<ExecutorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
}

impl CliOverrides {
    /// Build the flag layer from the individual CLI values.
    pub fn new(
        provider: Option<ProviderKind>,
        model: Option<String>,
        effort: Option<Effort>,
        executor: Option<ExecutorKind>,
        concurrency: Option<usize>,
    ) -> Self {
        Self {
            checks: CliChecksOverrides {
                provider,
                model,
                effort,
                executor,
                concurrency,
            },
        }
    }
}
