use std::path::{Path, PathBuf, absolute};

use miette::{IntoDiagnostic as _, Result};
use std::hash::Hasher as _;
use tokio::pin;
use tokio_stream::StreamExt as _;
use tracing::{error, trace};
use twox_hash::xxhash32::Hasher as XXHasher;

use crate::fs::stream_file;

/// This is the hashing algorithm for the 32-bit version of
/// `xxHash`.
pub struct XXHash32;

#[derive(Clone, Debug)]
pub struct FileHash32 {
    // This is guaranteed to be an absolute path.
    path: PathBuf,
    digest: u32,
    size: u64,
}

impl FileHash32 {
    pub fn new(path: PathBuf, digest: u32, size: u64) -> Self {
        Self { path, digest, size }
    }

    // Returns the 32-bit digest encoded as a hexademical string.
    pub fn digest(&self) -> String {
        format!("{:x}", self.digest)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

impl XXHash32 {
    /// The seed is used to initialize the hasher. We fix the
    /// seed to ensure we always get the same value between executions.
    const SEED: u32 = 0;

    pub async fn hash_file<P: AsRef<Path>>(filepath: P) -> Result<FileHash32> {
        error!("Hashing file");
        // Convert the path into an absolute path.
        let path = absolute(filepath).into_diagnostic()?;

        // Create the hasher.
        let mut hasher = XXHasher::with_seed(Self::SEED);
        let stream = stream_file(path.clone()).await;
        pin!(stream);

        // Hash the contents of file.
        while let Some(bytes) = stream.next().await {
            let bytes = bytes?;
            hasher.write(bytes.as_ref());
        }

        error!("Finished hashing file!");
        // Dump the output.
        Ok(FileHash32 {
            path,
            digest: hasher.finish_32(),
            size: hasher.total_len(),
        })
    }
}
