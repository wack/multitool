use std::path::{Path, PathBuf};

use derive_getters::Getters;
use ignore::WalkBuilder;
use miette::{IntoDiagnostic as _, Result, miette};

use tracing::debug;

use crate::manifest::manifest_filenames;

#[derive(Getters, Clone, Debug)]
pub(crate) struct CloudflareFileManifest {
    files: Vec<PathBuf>,
    root: PathBuf,
}

// TODO: Load in the `excludes` section of the Wranger.toml file and respect those.
// TODO: Determine if we should upload everything in `node_modules` or include
//       that as part of the build step.
impl CloudflareFileManifest {
    /// Build a new Manifest using the given root directory.
    pub async fn new<P: AsRef<Path>>(root: P) -> Result<Self> {
        debug!("Building Cloudflare manifest");
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
        let walker = WalkBuilder::new(directory.clone())
            .standard_filters(false)
            .build();

        for entry in walker {
            debug!("Processing entry: {:?}", entry.clone().unwrap().path());
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

            files.push(file_path);
        }

        debug!(
            "Finished building Cloudflare manifest with {} files",
            files.len()
        );
        Ok(Self {
            files,
            root: directory,
        })
    }
}
