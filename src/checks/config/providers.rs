//! Constructing the provider registry: one ready-to-use handle per provider
//! whose credential is present.
//!
//! This is the ticket's deliverable. We build at most three handles
//! (anthropic / openai / gemini), keyed by provider name, dispatching on
//! **provider name only** — never `router::from_model_string`, whose env/prefix
//! auto-detection is exactly the guessing a config layer must avoid. cersei has
//! no per-model types, so a handle is not pinned to a model; a future executor
//! supplies the model per request.
//!
//! Credentials are read here, at construction, straight from each provider's
//! native env var — never from the config file and never under `MULTI_`. A
//! provider is added iff its credential is present; a missing key is not an
//! error (those models simply can't be selected). The caller is responsible for
//! verifying the *configured* provider ended up available.

use std::collections::HashMap;

use cersei_provider::{Anthropic, Gemini, OpenAi, Provider};
use miette::{Result, miette};

use super::schema::{ProviderKind, ProvidersSection};

/// A provider name → constructed handle. At most three entries.
pub type ProviderRegistry = HashMap<String, Box<dyn Provider>>;

/// Read an env var, treating empty as unset.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// The credential for `provider` from its native env var(s), if present.
fn credential(provider: ProviderKind) -> Option<String> {
    match provider {
        ProviderKind::Anthropic => env_nonempty("ANTHROPIC_API_KEY"),
        ProviderKind::OpenAi => env_nonempty("OPENAI_API_KEY"),
        // Google's SDK convention: GOOGLE_API_KEY, falling back to GEMINI_API_KEY.
        ProviderKind::Gemini => {
            env_nonempty("GOOGLE_API_KEY").or_else(|| env_nonempty("GEMINI_API_KEY"))
        }
    }
}

/// A human-readable hint naming the env var(s) that supply `provider`'s
/// credential, for the "provider unavailable" error.
pub const fn credential_env_hint(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
        ProviderKind::OpenAi => "OPENAI_API_KEY",
        ProviderKind::Gemini => "GOOGLE_API_KEY or GEMINI_API_KEY",
    }
}

/// The native env var carrying `provider`'s base-URL override, if set. Used as a
/// fallback when no `[checks.providers.<name>].base_url` is configured.
fn native_base_url(provider: ProviderKind) -> Option<String> {
    let key = match provider {
        ProviderKind::Anthropic => "ANTHROPIC_BASE_URL",
        ProviderKind::OpenAi => "OPENAI_BASE_URL",
        ProviderKind::Gemini => "GEMINI_BASE_URL",
    };
    env_nonempty(key)
}

/// The base URL to use for `provider`: an explicit config override wins,
/// otherwise the provider's native env var, otherwise the cersei default.
fn resolve_base_url(provider: ProviderKind, overrides: &ProvidersSection) -> Option<String> {
    overrides
        .base_url(provider)
        .map(ToOwned::to_owned)
        .or_else(|| native_base_url(provider))
}

/// Construct the handle for `provider` with `key`, applying the optional
/// `base_url` before `.build()`. Dispatches on provider name only.
fn build_one(
    provider: ProviderKind,
    key: String,
    base_url: Option<String>,
) -> Result<Box<dyn Provider>> {
    let provider: Box<dyn Provider> = match provider {
        ProviderKind::Anthropic => {
            let mut b = Anthropic::builder().api_key(key);
            if let Some(url) = base_url {
                b = b.base_url(url);
            }
            Box::new(b.build().map_err(|e| miette!("anthropic provider: {e}"))?)
        }
        ProviderKind::OpenAi => {
            let mut b = OpenAi::builder().api_key(key);
            if let Some(url) = base_url {
                b = b.base_url(url);
            }
            Box::new(b.build().map_err(|e| miette!("openai provider: {e}"))?)
        }
        ProviderKind::Gemini => {
            let mut b = Gemini::builder().api_key(key);
            if let Some(url) = base_url {
                b = b.base_url(url);
            }
            Box::new(b.build().map_err(|e| miette!("gemini provider: {e}"))?)
        }
    };
    Ok(provider)
}

/// A per-provider handle factory: the resolved provider kind + credential +
/// base URL needed to mint a **fresh** [`Box<dyn Provider>`] on demand.
///
/// cersei's `Agent` takes an *owned* `Box<dyn Provider>` and checks run
/// concurrently (plus retries), so a single pre-built handle cannot be shared
/// across agents. The configuration phase resolves credentials once and hands
/// the executor this factory, which builds one handle per check run.
#[derive(Clone)]
pub struct ProviderFactory {
    kind: ProviderKind,
    key: String,
    base_url: Option<String>,
}

impl std::fmt::Debug for ProviderFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the credential.
        f.debug_struct("ProviderFactory")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl ProviderFactory {
    /// Mint a fresh provider handle.
    pub fn build(&self) -> Result<Box<dyn Provider>> {
        build_one(self.kind, self.key.clone(), self.base_url.clone())
    }
}

/// Build a [`ProviderFactory`] for `provider` if its credential is present,
/// resolving the same base-URL precedence as the registry. Returns `None` when
/// the provider has no credential (and therefore cannot be selected).
pub fn build_factory(
    provider: ProviderKind,
    overrides: &ProvidersSection,
) -> Option<ProviderFactory> {
    let key = credential(provider)?;
    let base_url = resolve_base_url(provider, overrides);
    Some(ProviderFactory {
        kind: provider,
        key,
        base_url,
    })
}

/// Build the registry: one handle per provider whose credential is present.
pub fn build_registry(overrides: &ProvidersSection) -> Result<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();
    for provider in [
        ProviderKind::Anthropic,
        ProviderKind::OpenAi,
        ProviderKind::Gemini,
    ] {
        let Some(key) = credential(provider) else {
            continue;
        };
        let base_url = resolve_base_url(provider, overrides);
        let handle = build_one(provider, key, base_url)?;
        registry.insert(provider.as_str().to_owned(), handle);
    }
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_skips_providers_without_credentials() {
        // We can't assume any keys are set in CI, but build_registry must never
        // error purely from missing keys — absent providers are just omitted.
        let overrides = ProvidersSection::default();
        let registry = build_registry(&overrides).expect("missing keys are not an error");
        // Only providers with a credential present appear; the map is a subset
        // of the three known providers.
        assert!(registry.len() <= 3);
        for name in registry.keys() {
            assert!(matches!(name.as_str(), "anthropic" | "openai" | "gemini"));
        }
    }
}
