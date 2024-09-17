use std::path::PathBuf;

use miette::Result;

use super::{DirectoryType, FileSystem};

/// One of the handful of known Wack-managed files. We expect these
/// file to be managed solely by the Wack CLI, so for correctness
/// we enumerate them individually.
pub(crate) trait MultiFile {
    /// The directory that this file belongs to.
    const DIR: DirectoryType;
    /// The name of the file, minus the extension.
    const NAME: &'static str;
    /// The file extension we expect to find. No dot is included.
    /// e.g. "json"
    const EXTENSION: &'static str;

    /// Return the expected path to the file. If this file's parent
    /// directory doesn't exist, it will be created.
    fn path(fs: &FileSystem) -> Result<PathBuf> {
        let filename = format!("{}.{}", Self::NAME, Self::EXTENSION);
        fs.init_dir(Self::DIR).map(|path| path.join(filename))
    }
}

pub(crate) trait MultiFileInstance {
    /// The directory that this file belongs to.
    const DIR: DirectoryType;

    /// The file extension we expect to find. No dot is included.
    /// e.g. "json"
    const EXTENSION: &'static str;

    /// The name of the file, minus the extension.
    fn name(&self) -> String;

    /// Return the expected path to the file. If this file's parent
    /// directory doesn't exist, it will be created.
    fn path(&self, fs: &FileSystem) -> Result<PathBuf> {
        let filename = format!("{}.{}", self.name(), Self::EXTENSION);
        fs.init_dir(Self::DIR).map(|path| path.join(filename))
    }
}

impl<T: MultiFile> MultiFileInstance for T {
    const DIR: DirectoryType = <Self as MultiFile>::DIR;

    const EXTENSION: &'static str = <Self as MultiFile>::EXTENSION;

    fn name(&self) -> String {
        Self::NAME.to_owned()
    }
}
