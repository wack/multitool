//! The hardcoded allowlist of valid model IDs, keyed by provider.
//!
//! The set of selectable models lives **in the binary**, not in config: a
//! configured `model` is only accepted if it appears here, and a future
//! per-check override will draw from the same table. These are the **concrete**
//! provider IDs the [`cersei_provider`] builders expect (e.g.
//! `claude-sonnet-4-6`), never the `claude` CLI's short aliases (`sonnet`).

use super::schema::ProviderKind;

/// Valid Anthropic model IDs. Also includes Fireworks model IDs usable via
/// Fireworks' Anthropic-compatible Messages endpoint (`[checks.providers.anthropic].base_url`
/// pointed at `https://api.fireworks.ai/inference`) — Fireworks speaks the same
/// wire format, so it slots into the `anthropic` provider rather than needing
/// its own [`ProviderKind`].
pub const ANTHROPIC_MODELS: &[&str] = &[
    "claude-opus-4-8",
    "claude-sonnet-4-6",
    "claude-haiku-4-5",
    "accounts/fireworks/routers/glm-5p1-fast",
];

/// Valid OpenAI model IDs.
pub const OPENAI_MODELS: &[&str] = &["gpt-4o", "gpt-4o-mini"];

/// Valid Gemini model IDs.
pub const GEMINI_MODELS: &[&str] = &["gemini-3.1-pro-preview"];

/// The default model for a provider when none is configured. Chosen to match
/// each `cersei_provider` builder's own default so behaviour is unsurprising.
pub const fn default_model(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Anthropic => "claude-sonnet-4-6",
        ProviderKind::OpenAi => "gpt-4o",
        ProviderKind::Gemini => "gemini-3.1-pro-preview",
    }
}

/// The allowlist of valid model IDs for a given provider.
pub const fn models_for(provider: ProviderKind) -> &'static [&'static str] {
    match provider {
        ProviderKind::Anthropic => ANTHROPIC_MODELS,
        ProviderKind::OpenAi => OPENAI_MODELS,
        ProviderKind::Gemini => GEMINI_MODELS,
    }
}

/// Whether `model` is a valid ID for `provider`.
pub fn is_valid_model(provider: ProviderKind, model: &str) -> bool {
    models_for(provider).contains(&model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_in_their_own_allowlist() {
        for provider in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::Gemini,
        ] {
            assert!(
                is_valid_model(provider, default_model(provider)),
                "default model for {provider:?} must be in its allowlist",
            );
        }
    }

    #[test]
    fn rejects_cli_aliases() {
        // The `claude` CLI alias `sonnet` is *not* a concrete ID and must not
        // validate — config carries the real ID.
        assert!(!is_valid_model(ProviderKind::Anthropic, "sonnet"));
        assert!(is_valid_model(ProviderKind::Anthropic, "claude-sonnet-4-6"));
    }

    #[test]
    fn models_are_not_cross_provider() {
        assert!(!is_valid_model(ProviderKind::OpenAi, "claude-sonnet-4-6"));
        assert!(!is_valid_model(ProviderKind::Anthropic, "gpt-4o"));
    }
}
