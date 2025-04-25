use std::path::{Path, PathBuf};

use miette::{IntoDiagnostic as _, Result};
use std::hash::Hasher as _;
use tokio::pin;
use tokio_stream::StreamExt as _;
use twox_hash::xxhash32::Hasher as XXHasher;

use crate::fs::stream_file;

/// This is the hashing algorithm for the 32-bit version of
/// `xxHash`.
pub struct XXHash32;

pub struct FileHash32 {
    pub path: PathBuf,
    pub digest: u32,
    pub size: u64,
}

impl XXHash32 {
    /// The seed is used to initialize the hasher. We fix the
    /// seed to ensure we always get the same value between executions.
    const SEED: u32 = 0;

    pub async fn hash_file<P: AsRef<Path>>(filepath: P) -> Result<FileHash32> {
        // Create the hasher.
        let mut hasher = XXHasher::with_seed(Self::SEED);
        let stream = stream_file(filepath.as_ref()).await;
        pin!(stream);

        // Hash the contents of file.
        while let Some(bytes) = stream.next().await {
            hasher.write(bytes?.as_ref());
        }

        // Dump the output.
        Ok(FileHash32 {
            path: filepath.as_ref().to_path_buf(),
            digest: hasher.finish_32(),
            size: hasher.total_len(),
        })
    }
}
