use std::{
    hash::Hasher,
    path::{Path, PathBuf, absolute},
};

use miette::{IntoDiagnostic as _, Result};
use tokio::pin;
use tokio_stream::StreamExt as _;
use twox_hash::xxhash32::Hasher as XXHasher;

use crate::fs::stream_file;

/// This is the hashing algorithm for the 32-bit version of
/// `xxHash`.
pub struct XXHash32;

pub struct FileHash32 {
    // This is guaranteed to be an absolute path.
    path: PathBuf,
    digest: u32,
    size: u64,
}

pub struct Hash32 {
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

impl Hash32 {
    pub fn new(digest: u32, size: u64) -> Self {
        Self { digest, size }
    }

    // Returns the 32-bit digest encoded as a hexademical string.
    pub fn digest(&self) -> String {
        format!("{:x}", self.digest)
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
        // Convert the path into an absolute path.
        let path = absolute(filepath).into_diagnostic()?;

        // Create the hasher.
        let mut hasher = Self::new_hasher();
        let stream = stream_file(path.clone()).await;
        pin!(stream);

        // Hash the contents of file.
        while let Some(bytes) = stream.next().await {
            hasher.write(bytes?.as_ref());
        }

        // Dump the output.
        Ok(FileHash32 {
            path,
            digest: hasher.finish_32(),
            size: hasher.total_len(),
        })
    }

    fn new_hasher() -> XXHasher {
        XXHasher::with_seed(Self::SEED)
    }

    pub async fn hash_str<S: AsRef<str>>(value: S) -> Hash32 {
        let input = value.as_ref().as_bytes();
        let size = input.len() as u64;
        // Hash the string in one go.
        let digest = XXHasher::oneshot(Self::SEED, input);
        Hash32 { size, digest }
    }
}
