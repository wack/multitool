use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// With the expectation that we will likely be making breaking changes,
/// we version the manifest schema (like how Docker Compose files come
/// with a schema version). Ideally, we would expect our manifest format
/// to be stable, but every growing project can expect to make breaking
/// changes during its genesis. Wrapping the manifest in a version enum
/// permits easy upgrades and is simple to program around.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(tag = "schema")]
#[serde(rename = "manifest")]
pub enum Manifest {
    #[serde(rename = "alpha")]
    Alpha(ManifestAlpha),
}

impl Manifest {
    pub fn dependencies(&self) -> IndexMap<String, Dependency> {
        match self {
            Manifest::Alpha(manifest) => manifest.dependencies.dependencies.clone(),
        }
    }

    pub fn account(&self) -> &str {
        match self {
            Manifest::Alpha(manifest) => &manifest.account,
        }
    }

    pub fn pkg_name(&self) -> &str {
        match self {
            Manifest::Alpha(manifest) => &manifest.pkg_name,
        }
    }

    pub fn version(&self) -> &str {
        match self {
            Manifest::Alpha(manifest) => &manifest.version,
        }
    }
}

/// The manifest format for the alpha release of Wack.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct ManifestAlpha {
    pub account: String,
    pub pkg_name: String,
    pub version: String,
    pub dependencies: DependencySection,
}

/// An object where the keys are the dependency name and the values
/// describe which version.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq, Debug, Default)]
pub struct DependencySection {
    #[serde(flatten)]
    pub dependencies: IndexMap<String, Dependency>,
}

/// Anticipating a struct variant (like Cargo), we use an enum here
/// in case we need to attach additional metadata to describe the dependency.
#[derive(Deserialize, Serialize, PartialEq, Eq, Debug, Clone)]
#[serde(untagged)]
pub enum Dependency {
    /// A Semver string
    Version(String),
}

#[cfg(test)]
mod tests {
    use super::{Dependency, DependencySection, Manifest, ManifestAlpha};
    use indexmap::indexmap;
    use pretty_assertions::assert_str_eq;

    #[test]
    fn parse_example1() {
        const RAW_MANIFEST: &str = r#"schema = "alpha"
account = "foobar"
pkg-name = "bazfizz"
version = "v1.0.0"

[dependencies]
foo = "v2.0.0"
bar = "v3.4.5"
"#;
        let observed: Manifest = toml::from_str(RAW_MANIFEST).expect("manifest not parsable");

        let expected = Manifest::Alpha(ManifestAlpha {
            account: "foobar".to_owned(),
            pkg_name: "bazfizz".to_owned(),
            version: "v1.0.0".to_owned(),
            dependencies: DependencySection {
                dependencies: indexmap! {
                    String::from("foo") => Dependency::Version("v2.0.0".to_owned()),
                    String::from("bar") => Dependency::Version("v3.4.5".to_owned()),
                },
            },
        });
        assert_eq!(expected, observed);

        // Convert it back to a string and compare.
        let roundtrip_manifest = toml::to_string_pretty(&expected).expect("must format to string");
        assert_str_eq!(roundtrip_manifest, RAW_MANIFEST.to_owned());
    }
}
