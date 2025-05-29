use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use derive_getters::Getters;
use ignore::WalkBuilder;
use miette::{IntoDiagnostic as _, Report, Result, miette};
use serde::{Serialize, Serializer};
use tokio::task::JoinSet;

use multi_core::{
    ManyError,
    hashing::{FileHash32, XXHash32},
};
use tracing::debug;
use tracing_subscriber::field::debug;

#[derive(Getters, Clone, Debug)]
pub(crate) struct CloudflareManifest {
    // For whatever reason, Cloudflare returns buckets
    // using the file's hash, not the file's name, so we
    // store the inverse of what you might expect.
    files: Vec<FileHash32>,
    root: PathBuf,
}

// We want data in the format:
// ```json
// {
//   "/package.json": { hash: "0xDEADBEEF", "size": 256 }
// }
// ```
impl Serialize for CloudflareManifest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize, Debug)]
        struct StructValue {
            hash: String,
            size: u64,
        }
        let mut manifest_data = HashMap::new();
        for file in self.files.iter() {
            // A FileHash32 contains the full path to the file, we need to make sure
            // that our root also has a full path so we can strip it correctly.
            let canonical_root = self
                .root
                .canonicalize()
                .expect("Root should be canonicalized");

            // debug!("{:?}", self.root);
            // debug!("{:?}", canonical_root);
            // debug!("{:?}", canonical_root.parent());

            // Strip the root path from the absolute path, since that's
            // the format CF wants it in.
            let mut key = file
                .path()
                .strip_prefix(&canonical_root)
                .expect("File path should be a subpath of the root")
                .display()
                .to_string();
            // If it doesn't start with a slash, give it one.
            if !key.starts_with('/') {
                key.insert(0, '/');
            }

            let value = StructValue {
                hash: file.digest(),
                size: file.size(),
            };
            manifest_data.insert(key, value);
        }
        debug!("Manifest data: {:?}", manifest_data);
        manifest_data.serialize(serializer)
    }
}

// TODO: Load in the `excludes` section of the Wranger.toml file and respect those.
// TODO: Determine if we should upload everything in `node_modules` or include
//       that as part of the build step.
impl CloudflareManifest {
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

        let mut threadpool = JoinSet::new();
        // Build the file tree walker.
        let walker = WalkBuilder::new(directory.clone())
            .standard_filters(false)
            .build();

        let mut errors = ManyError::default();
        for entry in walker {
            debug!("Processing entry: {:?}", entry);
            let file_entry = entry.into_diagnostic()?;

            // Ignore directories, we only want files.
            if file_entry.file_type().map_or(false, |ft| ft.is_dir()) {
                continue;
            }

            let file_path = file_entry.path().to_path_buf(); //.into_path()
            let future = XXHash32::hash_file(file_path);
            threadpool.spawn(future);
        }

        let files: Vec<_> = threadpool
            .join_all()
            .await
            .into_iter()
            .filter_map(|result| match result {
                Ok(digest) => Some(digest),
                Err(err) => {
                    errors.append(Report::msg(err));
                    None
                }
            })
            .collect();

        if !errors.is_empty() {
            debug!(
                "Encountered errors while building Cloudflare manifest: {:?}",
                errors
            );
            return Err(errors.into());
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

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr as _};

    use multi_core::hashing::FileHash32;
    use pretty_assertions::assert_str_eq;
    use serde_json::json;

    use super::CloudflareManifest;

    #[test]
    #[ignore = "This test is flaky since the JSON gets serialized out of order sometimes."]
    fn to_json() {
        let manifest = CloudflareManifest {
            root: PathBuf::from_str("/foo").unwrap(),
            files: vec![
                FileHash32::new(
                    PathBuf::from_str("/foo/package.json").unwrap(),
                    199u32,
                    5u64,
                ),
                FileHash32::new(PathBuf::from_str("/foo/README.md").unwrap(), 1000u32, 20u64),
            ],
        };
        let observed = serde_json::to_string_pretty(&manifest).unwrap();

        // NOTE: the base64 content is NOT included in the output
        let expected = serde_json::to_string_pretty(&json!({
            "/README.md": {
                "hash": "3e8",
                "size": 20,
            },
            "/package.json": {
                "hash": "c7",
                "size": 5,
            }
        }))
        .unwrap();
        assert_str_eq!(expected, observed);
    }
}
