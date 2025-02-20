use directories::ProjectDirs;
use file::StaticFile;
use miette::{Diagnostic, IntoDiagnostic, Result, miette};
use std::fs;
use thiserror::Error;

use std::{
    io::{BufReader, Read, Write},
    path::PathBuf,
};

pub(crate) use file::File;
pub(crate) use session::{Session, UserCreds};

use manifest::{JsonManifest, Manifest, TomlManifest};

mod file;
/// The schema and parsing code for the Wack.toml manifest file.
pub mod manifest;
mod session;

/// The name of the application as used on the filesystem for XDG conventions.
const APPLICATION_NAME: &str = "multi";

/// An abstraction over the user's filesystem ensuring mediated
/// access to the most commonly used files.
pub struct FileSystem {
    /// OS-specific file locations for standard operations,
    /// respecting $XDG_CONFIG and similar variables, and falling
    /// back to OS defaults.
    xdg_dirs: ProjectDirs,
}

impl FileSystem {
    pub fn new() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", APPLICATION_NAME);
        let xdg_dirs = dirs.ok_or_else(|| miette!("$HOME directory unavailable"))?;
        Ok(Self { xdg_dirs })
    }

    /// Returns `Ok(true)` if the file existed and was deleted.
    /// Returns `Ok(false)`` if the file did not exist.
    /// Returns `Err(_)`` if the file could not be deleted or there was another io error.
    pub(crate) fn delete_file<T: StaticFile>(&self) -> Result<bool> {
        // • Grab the path to the file.
        let path = T::static_path(self)?;
        // Remove the file but check the error.
        match std::fs::remove_file(path) {
            Ok(_) => Ok(true),
            Err(ref err) => match err.kind() {
                std::io::ErrorKind::NotFound => Ok(false),
                _ => Err(miette!("{}", err)),
            },
        }
    }

    /// Load the project manifest file, looking for manifests in
    /// priority order up the file hierarchy.
    pub fn project_manifest(&self) -> Result<Manifest> {
        // • Attempt to load a TOML manifest. Fallback to JSON.
        let toml_manifest = self.load_file(TomlManifest);
        let json_manifest = self.load_file(JsonManifest);
        let manifest_box = match (toml_manifest, json_manifest) {
            (Ok(manifest), _) => manifest,
            (Err(_), Ok(manifest)) => manifest,
            (Err(_), Err(_)) => return Err(ManifestMissing.into()),
        };
        Ok(manifest_box)
    }

    /// The project directory is the first directory with a Wack manifest
    /// staritng in the current directory and walking up the directory
    /// tree until one is observed.
    /// `Ok(Some(_))`` is returned when the file is found successfully.
    /// `Ok(None)` is returned when the file cannot be found.
    /// `Err` is returned when the file is found but some other error
    /// occurred, like the file could not be read due to insufficient
    /// permissions, or the pwd is outside of the bounds of the filesystem.
    /// This function only checks if the file exists, not if the file is valid.
    pub fn project_dir(&self) -> Result<Option<PathBuf>> {
        // • Check this directory for the wack manifest. If not found,
        //   traverse upward until found.
        let current_dir = std::env::current_dir().into_diagnostic()?;
        for dir in current_dir.ancestors() {
            for filename in crate::fs::manifest::manifest_filenames() {
                let candidate = dir.join(filename);
                if fs::metadata(&candidate).is_ok() {
                    return Ok(Some(dir.to_path_buf()));
                }
            }
        }
        Ok(None)
    }

    /// Open the file and deserialize it with serde.
    pub(crate) fn load_file<F: File>(&self, file: F) -> Result<F::Data> {
        match F::EXTENSION {
            "toml" => self.read_toml_file(file),
            "json" => self.read_json_file(file),
            _ => Err(miette!(
                "Extension unknown! Internal error. Please file this error as a bug."
            )),
        }
    }

    /// Open the file and deserialize it with serde.
    fn read_json_file<F: File>(&self, file: F) -> Result<F::Data> {
        // • Get the path to the file.
        let path = file.path(self)?;
        // • Open it as a byte stream, then deserialize those bytes.
        let file = std::fs::File::open(path).into_diagnostic()?;
        let reader = BufReader::new(file);

        // • Serialize the JSON contents of the file.
        serde_json::from_reader(reader).into_diagnostic()
    }

    /// Open the file and deserialize it with serde.
    fn read_toml_file<F: File>(&self, file: F) -> Result<F::Data> {
        // • Get the path to the file.
        let path = file.path(self)?;
        // • Open it as a byte stream, then deserialize those bytes.
        // TODO: Chain these errors together functionally.
        let mut buffer = String::new();
        let mut file = std::fs::File::open(path).into_diagnostic()?;
        file.read_to_string(&mut buffer).into_diagnostic()?;
        let document = toml::from_str(&buffer).into_diagnostic()?;
        Ok(document)
    }

    /// Open the file and deserialize it with serde.
    pub(crate) fn save_file<F: File>(&self, file: &F, blob: &F::Data) -> Result<()> {
        // • Get the path to the file.
        let path = file.path(self)?;
        // • Create the file if it doesn't exist.
        let mut file = std::fs::File::create(path).into_diagnostic()?;
        let marshalled = match F::EXTENSION {
            "toml" => toml::to_string_pretty(blob).into_diagnostic()?,
            "json" => serde_json::to_string_pretty(blob).into_diagnostic()?,
            _ => {
                return Err(miette!(
                    "Extension unknown! Internal error. Please file this error as a bug."
                ));
            }
        };
        file.write_all(marshalled.as_bytes()).into_diagnostic()?;
        file.sync_all().into_diagnostic()?;
        Ok(())
    }

    /// Returns the expected directory for this particular file type.
    fn dir(&self, typ: DirectoryType) -> Result<PathBuf> {
        match typ {
            DirectoryType::Cache => Ok(self.xdg_dirs.cache_dir().to_path_buf()),
            DirectoryType::Project => self.project_dir()?.ok_or(ManifestMissing.into()),
            DirectoryType::Pwd => std::env::current_dir().into_diagnostic(),
            DirectoryType::Data => Ok(self.xdg_dirs.data_dir().to_path_buf()),
        }
    }

    // TODO: Should we mock this function out using a virtual filesystem
    //       for testing?
    /// Ensure the given directory exists by recursively creating
    /// the necessary config dirs.
    fn init_dir(&self, typ: DirectoryType) -> Result<PathBuf> {
        let path_buf = self.dir(typ)?;
        let path = path_buf.as_path();

        // Build an error that displays the path and the OS error message
        // if we can't create the directory.
        let on_err = |err| {
            let displayable_path = path.display();
            // TODO: Turn this into a error with a diagnostic code.
            miette!("Could not create cache directory at {displayable_path}: {err}")
        };

        // Create the directory. This is a no-op if they already exist.
        std::fs::create_dir_all(path).map_err(on_err)?;
        // Return an owned path to the directory.
        Ok(path.to_path_buf())
    }
}

#[derive(Error, Debug, Diagnostic)]
#[error("Wack manifest file not found")]
struct ManifestMissing;

/// A shorthand for referring to one of the $XDG directories.
/// As we need additional directories, we'll add them to the enum.
pub enum DirectoryType {
    /// The directory for non-essential project files
    Cache,
    /// Persistent data lives here between runs.
    Data,
    /// The project directory is the dir that contains the manifest
    /// file relevant to the current operating context. It's usually
    /// the nearest wack.toml file, starting in the pwd and crawling
    /// up the directory tree until its found.
    Project,
    /// Sometimes, we need to create new files from scratch in the
    /// working directory. This extension is for cases when we're
    /// not interested in the project root. e.g. `wack init`
    Pwd,
}
