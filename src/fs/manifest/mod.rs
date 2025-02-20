use serde::{Deserialize, Serialize};

pub use schema::{Dependency, DependencySection, Manifest, ManifestAlpha};

use super::{DirectoryType, file::StaticFile};

mod schema;

/// This is the prefix of the manifest file name. e.g. wack.toml, wack.yml, and wack.json
/// are all prefixed with `wack`
const MANIFEST_PREFIX: &str = "wack";
/// This is the set of allowed filetypes for the manifest. Right now,
/// we only accept toml and Json files, but we could imagine accepting yml
/// files in the future.
const MANIFEST_EXTENSIONS: [&str; 2] = ["toml", "json"];

/// `manifest_filenames` returns an ordered list of legal filenames
/// for the manifest file.
//
// This is the cross-product of the MANIFEST_PREFIX and
// MANIFEST_EXTENSIONS constants.
pub(crate) fn manifest_filenames() -> [String; 2] {
    MANIFEST_EXTENSIONS.map(|ext| format!("{MANIFEST_PREFIX}.{ext}"))
}

#[derive(Serialize, Deserialize)]
pub struct TomlManifest;

/// A manifest that was originally loaded from a TOML file.
impl StaticFile for TomlManifest {
    const DIR: DirectoryType = DirectoryType::Project;
    const NAME: &'static str = MANIFEST_PREFIX;
    const EXTENSION: &'static str = MANIFEST_EXTENSIONS[0];

    type Data = Manifest;
}

/// A manifest that was originally loaded from a JSON file.
pub struct JsonManifest;

impl StaticFile for JsonManifest {
    const DIR: DirectoryType = DirectoryType::Project;
    const NAME: &'static str = MANIFEST_PREFIX;
    const EXTENSION: &'static str = MANIFEST_EXTENSIONS[1];

    type Data = Manifest;
}

/// This type is the same as `TomlManifest`, but in the context of
/// `wack new` the directory changes to pwd.
pub struct InitTomlManifest;

impl StaticFile for InitTomlManifest {
    const DIR: DirectoryType = DirectoryType::Pwd;
    const NAME: &'static str = TomlManifest::NAME;
    const EXTENSION: &'static str = TomlManifest::EXTENSION;

    type Data = Manifest;
}

/// This type is the same as `JsonManifest`, but in the context of
/// `wack new` the directory changes to pwd.
pub struct InitJsonManifest;

impl StaticFile for InitJsonManifest {
    const DIR: DirectoryType = DirectoryType::Pwd;
    const NAME: &'static str = JsonManifest::NAME;
    const EXTENSION: &'static str = JsonManifest::EXTENSION;

    type Data = Manifest;
}
