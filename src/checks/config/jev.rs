//! Resolving and validating the merged `[checks.jev]` table (TypeSafe/Jev
//! decision-engine settings).
//!
//! Defined **unconditionally** — not `#[cfg(feature = "jev")]` — so the schema
//! (see [`super::schema::JevSection`]) and this validation logic parse and
//! reject the same way whether or not the `jev` Cargo feature is compiled in.
//! A `[checks.jev]` table in `MultiTool.toml` never breaks a default build:
//! it round-trips through the ordinary `flag > env > file` figment merge (see
//! [`super::resolve_layers`]) exactly like every other `[checks]` field. Only
//! [`crate::checks::jev`] — the feature-gated HTTP client that actually talks
//! to TypeSafe — is missing from the default build; nothing here depends on it.
//!
//! Credentials are **not** resolved here. `TYPESAFE_API_KEY` is read straight
//! from the environment by the client itself, lazily, on the first request
//! (mirroring how the model providers' native env vars are handled in
//! [`super::providers`]) — never part of this merge, never under `MULTI_`.

use miette::{Result, miette};

use super::schema::JevSection;

// Everything below has no reader outside this file's own tests yet: nothing
// in the default build or the `jev`-feature build calls `resolve_jev` (or
// touches `JevConfig`/the defaults it falls back to) until MULTI-1825
// ("Decide `multi check` with Jev under the `jev` feature") wires it into
// `multi check`'s decision path. rustc's dead-code reachability analysis
// treats the whole subtree as unreached once its one entry point
// (`resolve_jev`) is, so each item below needs its own narrow allow rather
// than one at `resolve_jev` alone. Each is exercised today by this module's
// `#[cfg(test)]` block. Remove these once MULTI-1825 lands.

/// The default Jev model alias: TypeSafe's "most recent stable, official
/// release; the default in client SDKs" (<https://docs.typesafe.ai/models.md>).
#[allow(dead_code)] // see the note above; removed by MULTI-1825
pub const DEFAULT_MODEL: &str = "jev-latest";

/// The default confidence threshold gating a Jev-decided verdict. Values in
/// `(0, 1]` are accepted; see [`resolve_jev`].
#[allow(dead_code)] // see the note above; removed by MULTI-1825
pub const DEFAULT_THRESHOLD: f64 = 0.75;

/// The default TypeSafe SystemOne API origin
/// (<https://docs.typesafe.ai/api.md>). `checks.jev.base_url` is the single
/// source of truth for the endpoint, so this is the only hardcoded default —
/// there is no separate `TYPESAFE_BASE_URL` environment variable.
#[allow(dead_code)] // see the note above; removed by MULTI-1825
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";

/// The resolved, validated `[checks.jev]` settings: every field defaulted, and
/// `threshold` checked to be in `(0, 1]`. Carries no credential — see the
/// module docs.
#[allow(dead_code)] // see the note above; removed by MULTI-1825
#[derive(Debug, Clone, PartialEq)]
pub struct JevConfig {
    /// The Jev model ID or alias to request.
    pub model: String,
    /// The confidence threshold gating a Jev-decided verdict, in `(0, 1]`.
    pub threshold: f64,
    /// The TypeSafe SystemOne API origin (no trailing slash assumed; the
    /// client joins it with the endpoint path).
    pub base_url: String,
}

/// Resolve the merged `[checks.jev]` table into a [`JevConfig`], defaulting
/// unset fields and validating `threshold`.
///
/// # Errors
///
/// Returns a diagnostic if `threshold` is set but outside `(0, 1]`.
#[allow(dead_code)] // see the note above; removed by MULTI-1825
pub fn resolve_jev(section: &JevSection) -> Result<JevConfig> {
    let model = section
        .model
        .clone()
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let threshold = section.threshold.unwrap_or(DEFAULT_THRESHOLD);
    let base_url = section
        .base_url
        .clone()
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

    if !(threshold > 0.0 && threshold <= 1.0) {
        return Err(miette!(
            "checks.jev.threshold must be in (0, 1], got {threshold}"
        ));
    }

    Ok(JevConfig {
        model,
        threshold,
        base_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_when_section_is_empty() {
        let cfg = resolve_jev(&JevSection::default()).expect("defaults are valid");
        assert_eq!(cfg.model, DEFAULT_MODEL);
        assert_eq!(cfg.threshold, DEFAULT_THRESHOLD);
        assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn explicit_values_override_defaults() {
        let section = JevSection {
            model: Some("jev-preview".to_string()),
            threshold: Some(0.9),
            base_url: Some("https://jev.example".to_string()),
        };
        let cfg = resolve_jev(&section).expect("valid section");
        assert_eq!(cfg.model, "jev-preview");
        assert_eq!(cfg.threshold, 0.9);
        assert_eq!(cfg.base_url, "https://jev.example");
    }

    #[test]
    fn threshold_of_one_is_accepted() {
        // (0, 1] is inclusive of 1.
        let section = JevSection {
            threshold: Some(1.0),
            ..Default::default()
        };
        assert!(resolve_jev(&section).is_ok());
    }

    #[test]
    fn threshold_of_zero_is_rejected() {
        // (0, 1] excludes 0.
        let section = JevSection {
            threshold: Some(0.0),
            ..Default::default()
        };
        let err = resolve_jev(&section).unwrap_err().to_string();
        assert!(err.contains("threshold"), "got: {err}");
    }

    #[test]
    fn threshold_above_one_is_rejected() {
        let section = JevSection {
            threshold: Some(1.5),
            ..Default::default()
        };
        assert!(resolve_jev(&section).is_err());
    }

    #[test]
    fn negative_threshold_is_rejected() {
        let section = JevSection {
            threshold: Some(-0.1),
            ..Default::default()
        };
        assert!(resolve_jev(&section).is_err());
    }
}
