//! Locating and reading the `MultiTool.toml` file layer.
//!
//! Discovery, XDG/app-root resolution, and toml/json/jsonc parsing all reuse the
//! existing [`crate::fs`] machinery — these are just three more `StaticFile`
//! marker types pointing at the same `MultiTool.*` file the legacy manifest
//! reads. We deserialise into [`RootFileConfig`], which ignores the manifest's
//! own keys, so both readers can share one file.

use crate::fs::{DirectoryType, FileSystem, StaticFile};

use super::schema::RootFileConfig;

/// The shared file stem, matching the legacy manifest so a single
/// `MultiTool.toml` carries both rollout and checks config.
const FILE_STEM: &str = "MultiTool";

/// `MultiTool.toml`, read as checks config.
struct TomlChecksFile;
impl StaticFile for TomlChecksFile {
    type Data = RootFileConfig;
    const DIR: DirectoryType = DirectoryType::ApplicationRoot;
    const NAME: &'static str = FILE_STEM;
    const EXTENSION: &'static str = "toml";
}

/// `MultiTool.json`, read as checks config.
struct JsonChecksFile;
impl StaticFile for JsonChecksFile {
    type Data = RootFileConfig;
    const DIR: DirectoryType = DirectoryType::ApplicationRoot;
    const NAME: &'static str = FILE_STEM;
    const EXTENSION: &'static str = "json";
}

/// `MultiTool.jsonc`, read as checks config.
struct JsoncChecksFile;
impl StaticFile for JsoncChecksFile {
    type Data = RootFileConfig;
    const DIR: DirectoryType = DirectoryType::ApplicationRoot;
    const NAME: &'static str = FILE_STEM;
    const EXTENSION: &'static str = "jsonc";
}

/// Load the file layer, trying toml → json → jsonc in the discovered
/// application root. A missing or unreadable file is **not** an error: the file
/// layer is optional, so we fall back to an empty [`RootFileConfig`] and let the
/// env/flag layers (and hardcoded defaults) supply the values.
pub fn load_file_layer() -> RootFileConfig {
    let Ok(fs) = FileSystem::new() else {
        return RootFileConfig::default();
    };
    fs.load_file(TomlChecksFile)
        .or_else(|_| fs.load_file(JsonChecksFile))
        .or_else(|_| fs.load_file(JsoncChecksFile))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    // `figment::Jail`'s closure returns a large `Result`; unavoidable here.
    #![allow(clippy::result_large_err)]

    use super::*;
    use crate::checks::config::ProviderKind;
    use figment::Jail;

    #[test]
    fn reads_checks_table_alongside_legacy_manifest_keys() {
        Jail::expect_with(|jail| {
            // A single MultiTool.toml carrying both the legacy manifest's keys
            // and the new `[checks]` table. The checks loader must read its
            // table and ignore the rest rather than rejecting the file.
            jail.create_file(
                "MultiTool.toml",
                r#"
workspace = "wack"
application = "multitool"

[config.cloudflare]
project-dir = "src"

[checks]
provider = "openai"
model = "gpt-4o"
effort = "high"

[checks.providers.anthropic]
base_url = "https://anthropic.example"
"#,
            )?;

            let cfg = load_file_layer();
            assert_eq!(cfg.checks.provider, Some(ProviderKind::OpenAi));
            assert_eq!(cfg.checks.model.as_deref(), Some("gpt-4o"));
            assert_eq!(
                cfg.checks.providers.base_url(ProviderKind::Anthropic),
                Some("https://anthropic.example"),
            );
            Ok(())
        });
    }

    #[test]
    fn absent_file_yields_empty_layer() {
        Jail::expect_with(|_jail| {
            // Empty jail dir: no MultiTool.* present, so the file layer is empty
            // (not an error) and lower-precedence defaults take over.
            let cfg = load_file_layer();
            assert!(cfg.checks.provider.is_none());
            assert!(cfg.checks.model.is_none());
            Ok(())
        });
    }
}
