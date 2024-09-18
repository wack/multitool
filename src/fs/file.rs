use std::path::PathBuf;

use miette::Result;
use serde::{de::DeserializeOwned, Serialize};

use super::{DirectoryType, FileSystem};

/// One of the handful of known Wack-managed files. We expect these
/// file to be managed solely by the Wack CLI, so for correctness
/// we enumerate them individually.
pub(crate) trait File {
    /// The data type this file serializes to and from..
    type Data: DeserializeOwned + Serialize;

    /// The file extension we expect to find. Not dot is included.
    /// e.g. "json"
    const EXTENSION: &'static str;

    /// Return the expected path to the file. If this file's parent
    /// directory doesn't exist, it will be created.
    fn path(&self, fs: &FileSystem) -> Result<PathBuf>;
}

/// [StaticFiles] have a statically known name. For example,
/// `project.toml` has a statically known name, but a file whose name
/// is determined dynamically does not.
pub(crate) trait StaticFile {
    /// A data object that this file marshals to and from.
    type Data: DeserializeOwned + Serialize;
    /// The directory that this file belongs to.
    const DIR: DirectoryType;
    /// The name of the file, minus the extension.
    const NAME: &'static str;
    /// The file extension we expect to find. Not dot is included.
    /// e.g. "json"
    const EXTENSION: &'static str;

    fn static_path(fs: &FileSystem) -> Result<PathBuf> {
        let filename = format!("{}.{}", Self::NAME, Self::EXTENSION);
        fs.init_dir(Self::DIR).map(|path| path.join(filename))
    }
}

impl<T: StaticFile> File for T {
    type Data = T::Data;
    const EXTENSION: &'static str = T::EXTENSION;

    fn path(&self, fs: &FileSystem) -> Result<PathBuf> {
        Self::static_path(fs)
    }
}
