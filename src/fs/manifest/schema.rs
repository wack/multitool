use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::fs::FileSystem;

/// The project manifest only needs to be loaded once, so we
/// cache it as a global singleton.
static PROJECT_MANIFEST: OnceLock<Manifest> = OnceLock::new();

/// Return the project's manifest file, or an empty manifest if none
/// is found. Use the globally available cached file.
pub fn project_manifest() -> &'static Manifest {
    PROJECT_MANIFEST.get_or_init(Manifest::load_or_default)
}

/// With the expectation that we will likely be making breaking changes,
/// we version the manifest schema (like how Docker Compose files come
/// with a schema version). Ideally, we would expect our manifest format
/// to be stable, but every growing project can expect to make breaking
/// changes during its genesis. Wrapping the manifest in a version enum
/// permits easy upgrades and is simple to program around.
#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq, Debug)]
pub struct Manifest {
    workspace: Option<String>,
    application: Option<String>,
}

impl Manifest {
    /// Attempts to read a config file for this project, and
    /// returns an empty manifest file if none is found.
    pub(crate) fn load_or_default() -> Self {
        if let Ok(fs) = FileSystem::new() {
            if let Ok(manifest) = fs.project_manifest() {
                return manifest;
            }
        }
        Self::default()
        // FileSystem::new()
        //     .and_then(|fs| fs.project_manifest())
        //     .unwrap_or_default()
    }

    pub fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }

    pub fn application(&self) -> Option<&str> {
        self.application.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::Manifest;
    use pretty_assertions::assert_str_eq;

    #[test]
    fn parse_example1() {
        const RAW_MANIFEST: &str = r#"workspace = "wack"
application = "multitool"
"#;
        let observed: Manifest = toml::from_str(RAW_MANIFEST).expect("manifest not parsable");

        let expected = Manifest {
            workspace: Some("wack".to_owned()),
            application: Some("multitool".to_owned()),
        };
        assert_eq!(expected, observed);

        // Convert it back to a string and compare.
        let roundtrip_manifest = toml::to_string_pretty(&expected).expect("must format to string");
        assert_str_eq!(roundtrip_manifest, RAW_MANIFEST.to_owned());
    }
}
