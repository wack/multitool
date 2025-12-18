use std::path::{Path, PathBuf};

use derive_getters::Getters;
use ignore::{
    Walk, WalkBuilder,
    types::{Types, TypesBuilder},
};
use miette::{IntoDiagnostic as _, Result, miette};
use tracing::debug;

use crate::fs::manifest::manifest_filenames;

#[cfg(feature = "vercel")]
use sha1::{Sha1, Digest};

#[derive(Clone, Debug)]
pub(crate) struct VercelFileEntry {
    path: PathBuf,
    sha1: String,
}

impl VercelFileEntry {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn sha1(&self) -> &str {
        &self.sha1
    }
}

#[derive(Getters, Clone, Debug)]
pub(crate) struct VercelFileManifest {
    files: Vec<VercelFileEntry>,
}

impl VercelFileManifest {
    /// Build a new Manifest using the given root directory.
    /// Calculates SHA1 hash for each file.
    #[cfg(feature = "vercel")]
    pub async fn new<P: AsRef<Path>>(root: P) -> Result<Self> {
        debug!("Building Vercel manifest with SHA1 hashes");
        let directory = root.as_ref().to_path_buf();

        // We must provide a valid directory.
        if !directory.metadata().is_ok_and(|meta| meta.is_dir()) {
            return Err(miette!(format!(
                "The provided path `{}` is not a valid directory",
                directory.display()
            )));
        }

        let mut files = Vec::new();
        let manifest_filenames = manifest_filenames();

        // Build the file tree walker.
        let walker = walk_builder(directory.clone());

        for entry in walker {
            debug!("Processing entry: {:?}", entry.as_ref().ok().map(|e| e.path()));
            let file_entry = entry.into_diagnostic()?;

            // Ignore directories, we only want files.
            if file_entry.file_type().map_or(false, |ft| ft.is_dir()) {
                continue;
            }

            let file_path = file_entry.path().to_path_buf();

            // Skip files with names that match manifest filenames
            if let Some(filename) = file_path.file_name().and_then(|n| n.to_str()) {
                if manifest_filenames.contains(&filename.to_string()) {
                    continue;
                }
            }

            // Calculate SHA1 hash
            let file_bytes = tokio::fs::read(&file_path).await.into_diagnostic()?;
            let mut hasher = Sha1::new();
            hasher.update(&file_bytes);
            let hash_result = hasher.finalize();
            let sha1 = hex::encode(hash_result);

            files.push(VercelFileEntry {
                path: file_path,
                sha1,
            });
        }

        debug!(
            "Finished building Vercel manifest with {} files",
            files.len()
        );

        // A manifest with no files should be an error
        if files.is_empty() {
            return Err(miette!(format!(
                "No files found in directory `{}` to upload",
                directory.display()
            )));
        }

        Ok(Self { files })
    }

    #[cfg(not(feature = "vercel"))]
    pub async fn new<P: AsRef<Path>>(_root: P) -> Result<Self> {
        Err(miette!("Vercel feature is not enabled. Enable the 'vercel' feature flag to use Vercel functionality."))
    }
}

/// Build a file loader that loads web development files.
fn types_matches() -> Types {
    let mut builder = TypesBuilder::new();
    builder.add_defaults();
    builder
        .select("ts")
        .select("js")
        .select("css")
        .select("html")
        .select("json");
    builder.build().unwrap()
}

/// Builder the Walker that walks the file tree looking for files.
/// It obeys the type filters we created.
fn walk_builder(dir: PathBuf) -> Walk {
    let types = types_matches();
    WalkBuilder::new(dir)
        .standard_filters(true)
        .parents(false)
        .types(types)
        .build()
}
