use miette::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub use schema::{Dependency, DependencySection, ManifestAlpha, ManifestContents};

use super::{DirectoryType, FileSystem, MultiFile};

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

/// A Manifest represents a valid, parsed manifest file, and also
/// exposes metadata about the manifest file, like where it lives on
/// disk.
pub trait Manifest {
    /// returns the contents of the parsed manifest.
    fn contents(&self) -> &ManifestContents;
    /// returns the absolute path on disk that this manifest was loaded from.
    fn location(&self, fs: &FileSystem) -> Result<PathBuf>;
}

impl Manifest for TomlManifest {
    fn contents(&self) -> &ManifestContents {
        &self.0
    }

    fn location(&self, fs: &FileSystem) -> Result<PathBuf> {
        Self::path(fs)
    }
}

impl Manifest for JsonManifest {
    fn contents(&self) -> &ManifestContents {
        &self.0
    }

    fn location(&self, fs: &FileSystem) -> Result<PathBuf> {
        Self::path(fs)
    }
}

#[derive(Serialize, Deserialize)]
pub struct TomlManifest(ManifestContents);

/// A manifest that was originally loaded from a TOML file.
impl MultiFile for TomlManifest {
    const DIR: DirectoryType = DirectoryType::Project;
    const NAME: &'static str = MANIFEST_PREFIX;
    const EXTENSION: &'static str = MANIFEST_EXTENSIONS[0];
}

/// A manifest that was originally loaded from a JSON file.
#[derive(Serialize, Deserialize)]
pub struct JsonManifest(ManifestContents);

impl MultiFile for JsonManifest {
    const DIR: DirectoryType = DirectoryType::Project;
    const NAME: &'static str = MANIFEST_PREFIX;
    const EXTENSION: &'static str = MANIFEST_EXTENSIONS[1];
}

/// This type is the same as `TomlManifest`, but in the context of
/// `wack new` the directory changes to pwd.
#[derive(Serialize, Deserialize)]
pub struct InitTomlManifest(pub ManifestContents);

impl MultiFile for InitTomlManifest {
    const DIR: DirectoryType = DirectoryType::Pwd;
    const NAME: &'static str = TomlManifest::NAME;
    const EXTENSION: &'static str = TomlManifest::EXTENSION;
}

/// This type is the same as `JsonManifest`, but in the context of
/// `wack new` the directory changes to pwd.
#[derive(Serialize, Deserialize)]
pub struct InitJsonManifest(pub ManifestContents);

impl MultiFile for InitJsonManifest {
    const DIR: DirectoryType = DirectoryType::Pwd;
    const NAME: &'static str = JsonManifest::NAME;
    const EXTENSION: &'static str = JsonManifest::EXTENSION;
}

#[cfg(test)]
mod tests {
    use super::Manifest;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Manifest);
}
