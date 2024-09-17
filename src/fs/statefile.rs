use serde::{Deserialize, Serialize};

use crate::fs::DirectoryType;
use crate::state::State;

use super::file::MultiFileInstance;

#[derive(Serialize, Deserialize)]
pub struct Statefile {
    pub(super) project_name: String,
    pub(super) state: State,
}

/// A manifest that was originally loaded from a TOML file.
impl MultiFileInstance for Statefile {
    const DIR: DirectoryType = DirectoryType::Cache;
    const EXTENSION: &'static str = "json";

    fn name(&self) -> String {
        format!("{}-statefile", &self.project_name)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        fs::{statefile::Statefile, FileSystem, MultiFile},
        state::{ResourcePrototype, ResourceRecord, State},
    };

    use super::{Dependency, DependencySection, ManifestAlpha, ManifestContents};
    use indexmap::indexmap;
    use miette::Result;
    use pretty_assertions::assert_str_eq;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn write_example() -> Result<()> {
        let mut state = State::empty();
        let statefile = Statefile {
            project_name: "Test".to_owned(),
            state,
        };

        let resource1_prototype = ResourcePrototype::new(Uuid::new_v4(), json!({}));
        let resource1 = ResourceRecord::new(&resource1_prototype, json!({}));

        state.add(resource1);

        let fs = FileSystem::new()?;

        fs.save_file(&state)?;

        let read_state = fs.load_file::<Statefile>();

        const RAW_MANIFEST: &str = r#"schema = "alpha"
account = "foobar"
pkg-name = "bazfizz"
version = "v1.0.0"

[dependencies]
foo = "v2.0.0"
bar = "v3.4.5"
"#;
        let observed: ManifestContents =
            toml::from_str(RAW_MANIFEST).expect("manifest not parsable");

        let expected = ManifestContents::Alpha(ManifestAlpha {
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
