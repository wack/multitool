use directories::ProjectDirs;
use miette::{Diagnostic, IntoDiagnostic, Result, miette};
use std::fs;
use thiserror::Error;

use std::{
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

pub(crate) use file::File;
// Re-exported so config layers outside the `fs` module (e.g. `checks::config`)
// can declare their own `StaticFile` marker types and reuse the loader/discovery.
pub(crate) use file::StaticFile;
pub(crate) use manifest::application_manifest;
pub(crate) use session::{Session, SessionFile, UserCreds};
pub(crate) use wrangler::{JsonWranglerFile, JsoncWranglerFile, TomlWranglerFile};

use manifest::{JsonManifest, Manifest, TomlManifest};
use wrangler::Wrangler;

mod file;
/// The schema and parsing code for the Wack.toml manifest file.
pub mod manifest;
mod session;
pub mod wrangler;

/// The name of the application as used on the filesystem for XDG conventions.
const APPLICATION_NAME: &str = "multi";

/// An abstraction over the user's filesystem ensuring mediated
/// access to the most commonly used files.
#[derive(Clone)]
pub struct FileSystem {
    /// OS-specific file locations for standard operations,
    /// respecting $XDG_CONFIG and similar variables, and falling
    /// back to OS defaults.
    xdg_dirs: ProjectDirs,
}

#[derive(Debug, Error, Diagnostic)]
#[error("$HOME directory unavailable")]
pub struct MissingHomeDirectory;

impl FileSystem {
    pub fn new() -> Result<Self, MissingHomeDirectory> {
        let dirs = ProjectDirs::from("", "", APPLICATION_NAME);
        let xdg_dirs = dirs.ok_or(MissingHomeDirectory)?;
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

    /// Load the application manifest file, looking for manifests in
    /// priority order up the file hierarchy.
    pub fn application_manifest(&self) -> Result<Manifest, ManifestMissing> {
        // Attempt to load a TOML manifest. Fallback to JSON.
        let toml_manifest = self.load_file(TomlManifest);
        let json_manifest = self.load_file(JsonManifest);
        let manifest_box = match (toml_manifest, json_manifest) {
            (Ok(manifest), _) => manifest,
            (Err(_), Ok(manifest)) => manifest,
            (Err(_), Err(_)) => return Err(ManifestMissing),
        };
        Ok(manifest_box)
    }

    /// Load the wrangler configuration file, looking for TOML, JSON, and JSONC formats
    pub fn wrangler_config(&self) -> Result<Wrangler, WranglerMissing> {
        // Attempt to load a TOML wrangler file. Fallback to JSON, then JSONC.
        let toml_wrangler = self.load_file(TomlWranglerFile);
        let json_wrangler = self.load_file(JsonWranglerFile);
        let jsonc_wrangler = self.load_file(JsoncWranglerFile);
        let wrangler_config = match (toml_wrangler, json_wrangler, jsonc_wrangler) {
            (Ok(wrangler), _, _) => wrangler,
            (Err(_), Ok(wrangler), _) => wrangler,
            (Err(_), Err(_), Ok(wrangler)) => wrangler,
            (Err(_), Err(_), Err(_)) => return Err(WranglerMissing),
        };
        Ok(wrangler_config)
    }

    /// The application directory is the first directory with a MultiTool manifest
    /// starting in the current directory and walking up the directory
    /// tree until one is observed.
    /// `Ok(Some(_))`` is returned when the file is found successfully.
    /// `Ok(None)` is returned when the file cannot be found.
    /// `Err` is returned when the file is found but some other error
    /// occurred, like the file could not be read due to insufficient
    /// permissions, or the pwd is outside of the bounds of the filesystem.
    /// This function only checks if the file exists, not if the file is valid.
    pub fn application_dir(&self) -> Result<Option<PathBuf>> {
        let current_dir = std::env::current_dir().into_diagnostic()?;
        Ok(find_manifest_root(&current_dir))
    }

    /// Open the file and deserialize it with serde.
    pub(crate) fn load_file<F: File>(&self, file: F) -> Result<F::Data> {
        match F::EXTENSION {
            "toml" => self.read_toml_file(file),
            "json" => self.read_json_file(file),
            "jsonc" => self.read_jsonc_file(file),
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

    /// Open the file and deserialize it with serde (JSONC with comments support).
    fn read_jsonc_file<F: File>(&self, file: F) -> Result<F::Data> {
        // • Get the path to the file.
        let path = file.path(self)?;
        // • Open it as a byte stream, then deserialize those bytes.
        let mut buffer = String::new();
        let mut file = std::fs::File::open(path).into_diagnostic()?;
        file.read_to_string(&mut buffer).into_diagnostic()?;
        let document = serde_json5::from_str(&buffer).into_diagnostic()?;
        Ok(document)
    }

    /// Store the file, using its canonical path.
    pub(crate) fn save_file<F: File>(&self, file: &F, blob: &F::Data) -> Result<()> {
        // • Get the path to the file.
        let path = file.path(self)?;
        // • Create the file if it doesn't exist.
        let mut file = std::fs::File::create(path).into_diagnostic()?;
        let marshalled = match F::EXTENSION {
            "toml" => toml::to_string_pretty(blob).into_diagnostic()?,
            "json" => serde_json::to_string_pretty(blob).into_diagnostic()?,
            "jsonc" => serde_json5::to_string(blob).into_diagnostic()?,
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
            DirectoryType::ApplicationRoot => self.application_dir()?.ok_or(ManifestMissing.into()),
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

/// Walk upward from `start` (inclusive) for the nearest ancestor directory
/// containing a MultiTool manifest (`MultiTool.toml` / `.json` / `.jsonc`).
/// Returns `None` if no ancestor of `start` has one.
///
/// This is the walk [`FileSystem::application_dir`] performs, factored out so
/// callers that need it from a directory other than the process's current
/// directory can reuse it directly (MULTI-1834: each requirements file
/// resolves its own repository root from its own location, never from the
/// pwd or the scan directory `multi check` was invoked with).
///
/// `start` is resolved to an absolute path internally (via
/// [`std::path::absolute`], a lexical operation — no symlinks are resolved
/// and nothing is required to exist) before walking. This matters for a
/// **relative** `start`: `Path::ancestors()` on a relative path terminates at
/// the *empty* path rather than climbing to the filesystem root, so a
/// relative walk can never see a manifest above the directory `start` was
/// relative to, and `"".join(filename)` silently resolves against the
/// process's current directory instead of correctly reporting "no ancestor
/// found". Absolutizing first fixes both: the walk always reaches the real
/// filesystem root, and this function can never return the empty path. A
/// failure to resolve `start` (e.g. the process's current directory is
/// unavailable) is treated as "no manifest found" rather than propagated,
/// matching this function's `Option`-shaped, best-effort contract.
pub(crate) fn find_manifest_root(start: &Path) -> Option<PathBuf> {
    let start = std::path::absolute(start).ok()?;
    for dir in start.ancestors() {
        for filename in crate::fs::manifest::manifest_filenames() {
            let candidate = dir.join(filename);
            if fs::metadata(&candidate).is_ok() {
                return Some(dir.to_path_buf());
            }
        }
    }
    None
}

#[derive(Error, Debug, Diagnostic)]
#[error("MultiTool manifest file not found")]
pub struct ManifestMissing;

#[derive(Error, Debug, Diagnostic)]
#[error("Wrangler JSON or TOML file required for Cloudflare Workers")]
pub struct WranglerMissing;

/// A shorthand for referring to one of the $XDG directories.
/// As we need additional directories, we'll add them to the enum.
pub enum DirectoryType {
    /// The directory for non-essential project files
    Cache,
    /// Persistent data lives here between runs.
    // We will probably need this later, like when we need to check
    // version expiration dates without phoning home.
    #[allow(dead_code)]
    Data,
    /// The application root directory is the dir that contains the manifest
    /// file relevant to the current operating context. It's usually
    /// the directory containing the nearest `MultiTool.toml` file, starting
    /// in the pwd and crawling up the directory tree until its found.
    ApplicationRoot,
    /// Sometimes, we need to create new files from scratch in the
    /// working directory. This extension is for cases when we're
    /// not interested in the application root. e.g. `multi init`
    Pwd,
}

#[cfg(test)]
mod tests {
    // `figment::Jail`'s closure returns a large `Result`; unavoidable here.
    #![allow(clippy::result_large_err)]

    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn manifest_beside_the_file_is_the_root() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MultiTool.toml"), "").unwrap();
        assert_eq!(
            find_manifest_root(dir.path()),
            Some(dir.path().to_path_buf())
        );
    }

    #[test]
    fn manifest_several_levels_up_is_found() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MultiTool.toml"), "").unwrap();
        let nested = dir.path().join("a/b/c");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_manifest_root(&nested), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn nested_manifests_the_nearest_wins() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MultiTool.toml"), "").unwrap();
        let service = dir.path().join("services/keystore");
        fs::create_dir_all(&service).unwrap();
        fs::write(service.join("MultiTool.toml"), "").unwrap();
        assert_eq!(find_manifest_root(&service), Some(service.clone()));
    }

    #[test]
    fn json_and_jsonc_manifests_count() {
        for filename in ["MultiTool.json", "MultiTool.jsonc"] {
            let dir = TempDir::new().unwrap();
            fs::write(dir.path().join(filename), "").unwrap();
            assert_eq!(
                find_manifest_root(dir.path()),
                Some(dir.path().to_path_buf()),
                "extension: {filename}"
            );
        }
    }

    #[test]
    fn resolution_is_unaffected_by_process_cwd() {
        use figment::Jail;
        Jail::expect_with(|jail| {
            // The process cwd (the jail) holds its own manifest — a decoy that
            // must not influence resolution starting from an unrelated `start`.
            jail.create_file("MultiTool.toml", "")?;

            let real = TempDir::new().unwrap();
            fs::write(real.path().join("MultiTool.toml"), "").unwrap();
            let start = real.path().join("a/b");
            fs::create_dir_all(&start).unwrap();

            assert_eq!(find_manifest_root(&start), Some(real.path().to_path_buf()));
            Ok(())
        });
    }

    /// Regression test for a code-review blocker on MULTI-1834: `start.ancestors()`
    /// on a *relative* path terminates at the empty path rather than climbing to
    /// the filesystem root, so a relative `start` (e.g. `multi check`'s default
    /// scan directory `.`, or a relative subdirectory argument) could never see a
    /// manifest above wherever it was relative to, and — worse — the empty-path
    /// ancestor's manifest check silently resolved against the process's cwd,
    /// which could return `Some(PathBuf::new())`. `find_manifest_root` must
    /// absolutize its input so a relative `start` climbs correctly and the
    /// result is never the empty path.
    #[test]
    fn relative_start_climbs_to_a_manifest_above_and_is_never_empty() {
        use figment::Jail;
        Jail::expect_with(|jail| {
            jail.create_file("MultiTool.toml", "")?;
            fs::create_dir_all("a/b/c").unwrap();

            let found = find_manifest_root(Path::new("a/b/c"));
            assert_eq!(found, Some(jail.directory().to_path_buf()));
            assert_ne!(found, Some(PathBuf::new()));
            Ok(())
        });
    }
}
