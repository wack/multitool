use serde::{Deserialize, Serialize};

/// With the expectation that we will likely be making breaking changes,
/// we version the manifest schema (like how Docker Compose files come
/// with a schema version). Ideally, we would expect our manifest format
/// to be stable, but every growing project can expect to make breaking
/// changes during its genesis. Wrapping the manifest in a version enum
/// permits easy upgrades and is simple to program around.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
pub struct Manifest {
    workspace: Option<String>,
    application: Option<String>,
}

impl Manifest {
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
