//! Per-check copy-on-write filesystem sandboxing (M3).
//!
//! Each check runs inside a CoW clone of the working tree so an agent can read
//! and modify files freely without corrupting the real working directory. The
//! abstraction is a boxed trait object (mirroring `BoxedIngress` etc.); the
//! concrete implementation is selected per platform via `cfg`. macOS ships an
//! APFS `clonefile` implementation and Linux a reflink (`FICLONE`)
//! implementation; any remaining target gets a stub that errors, so the crate
//! still builds everywhere.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use miette::Result;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod fallback;

/// A copy-on-write sandbox factory.
#[async_trait]
pub trait Sandbox: Send + Sync {
    /// Create a CoW clone of `source` and return a handle whose path is the
    /// sandbox root.
    async fn create(&self, source: &Path) -> Result<SandboxHandle>;
}

/// A boxed [`Sandbox`] for dynamic dispatch (the OS-injection seam).
pub type BoxedSandbox = Box<dyn Sandbox + Send + Sync>;

/// A live sandbox. Its [`SandboxHandle::path`] is an independent clone root; the
/// clone is removed when the handle is dropped (RAII teardown).
pub struct SandboxHandle {
    root: PathBuf,
    /// Owns the temp directory containing the clone; dropping it removes the
    /// clone. `None` only for hand-constructed handles in tests.
    _temp: Option<tempfile::TempDir>,
}

impl SandboxHandle {
    /// The sandbox root directory (use as the agent's working directory).
    pub fn path(&self) -> &Path {
        &self.root
    }
}

/// Select the platform sandbox implementation.
///
/// macOS → the APFS `clonefile` CoW sandbox. Linux → the reflink (`FICLONE`) CoW
/// sandbox. Any other platform → an unsupported stub that fails with a clear
/// diagnostic (Windows CoW support is tracked under *Future work*).
pub fn select_sandbox() -> BoxedSandbox {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::ApfsSandbox::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::ReflinkSandbox::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Box::new(fallback::UnsupportedSandbox)
    }
}

/// A test-only sandbox that does not clone: it hands back the source path
/// directly (agents in tests are fakes that never touch the filesystem).
#[cfg(test)]
pub struct NoopSandbox;

#[cfg(test)]
#[async_trait]
impl Sandbox for NoopSandbox {
    async fn create(&self, source: &Path) -> Result<SandboxHandle> {
        Ok(SandboxHandle {
            root: source.to_path_buf(),
            _temp: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Sandbox);

    #[tokio::test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    async fn clone_is_independent_of_source() {
        use std::fs;
        let src = tempfile::TempDir::new().unwrap();
        fs::write(src.path().join("a.txt"), "original").unwrap();
        fs::create_dir(src.path().join("nested")).unwrap();
        fs::write(src.path().join("nested/b.txt"), "b").unwrap();

        let sandbox = select_sandbox();
        let handle = sandbox.create(src.path()).await.unwrap();

        // The clone has the files.
        assert_eq!(
            fs::read_to_string(handle.path().join("a.txt")).unwrap(),
            "original"
        );
        assert_eq!(
            fs::read_to_string(handle.path().join("nested/b.txt")).unwrap(),
            "b"
        );

        // Writing inside the sandbox does not touch the source.
        fs::write(handle.path().join("a.txt"), "modified").unwrap();
        assert_eq!(
            fs::read_to_string(src.path().join("a.txt")).unwrap(),
            "original"
        );

        // Teardown removes the clone.
        let clone_path = handle.path().to_path_buf();
        drop(handle);
        assert!(!clone_path.exists());
    }
}
