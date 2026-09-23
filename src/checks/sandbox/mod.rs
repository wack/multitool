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
use std::sync::Arc;

use async_trait::async_trait;
use miette::Result;
use tokio::sync::OnceCell;

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

/// A lazily-created CoW sandbox for one check attempt (MULTI-1818). Wraps the
/// injected [`Sandbox`] factory and the source directory (a requirement's
/// repository root, [`crate::checks::model::Requirement::root`]) to clone
/// from — but does not clone anything until asked.
///
/// [`SandboxLease::acquire`] creates the clone on its first call and hands
/// back that same clone on any later call, so a check whose evidence is
/// settled without an agent (the Jev decision engine's in-host replay,
/// MULTI-1825) never pays for a clone: an executor that never calls
/// `acquire` triggers zero [`Sandbox::create`] calls. The clone is removed
/// when the lease — and, transitively, the
/// [`AgentRunRequest`](crate::checks::executor::AgentRunRequest) carrying it
/// — is dropped, the same RAII teardown the eager `sandbox.create` call used
/// to get for free.
pub struct SandboxLease {
    sandbox: Arc<dyn Sandbox + Send + Sync>,
    source: PathBuf,
    handle: OnceCell<SandboxHandle>,
}

impl SandboxLease {
    /// Wrap `sandbox` for lazily cloning `source`.
    pub fn new(sandbox: Arc<dyn Sandbox + Send + Sync>, source: PathBuf) -> Self {
        Self {
            sandbox,
            source,
            handle: OnceCell::new(),
        }
    }

    /// Acquire the sandbox, cloning the source directory on the first call.
    /// Later calls on the same lease reuse that clone rather than creating
    /// another one, so a lease is safe to acquire more than once within an
    /// attempt.
    pub async fn acquire(&self) -> Result<&Path> {
        self.handle
            .get_or_try_init(|| self.sandbox.create(&self.source))
            .await
            .map(SandboxHandle::path)
    }
}

/// Select the sandbox implementation.
///
/// Sandboxing is opt-in (`--sandbox`): check agents only get read-only tools
/// (Read, Grep, Glob — each jailed to the agent's working directory — plus the
/// judge tool), so they have no way to mutate the tree they inspect. With
/// `enabled` false this returns [`NoopSandbox`], which hands back the source
/// directory itself and clones nothing.
///
/// With `enabled` true: macOS → the APFS `clonefile` CoW sandbox. Linux → the
/// reflink (`FICLONE`) CoW sandbox. Any other platform → an unsupported stub
/// that fails with a clear diagnostic (Windows CoW support is tracked under
/// *Future work*).
pub fn select_sandbox(enabled: bool) -> BoxedSandbox {
    if !enabled {
        return Box::new(NoopSandbox);
    }
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

/// A sandbox that does not clone: it hands back the source path directly. The
/// default when `--sandbox` is not passed (see [`select_sandbox`]), and used
/// by tests whose agents are fakes that never touch the filesystem.
pub struct NoopSandbox;

#[async_trait]
impl Sandbox for NoopSandbox {
    async fn create(&self, source: &Path) -> Result<SandboxHandle> {
        Ok(SandboxHandle {
            root: source.to_path_buf(),
            _temp: None,
        })
    }
}

/// A test-only sandbox that, like [`NoopSandbox`], doesn't clone — but records
/// every source path it was asked to create a sandbox for, in call order. Used
/// to assert *what* execution clones (MULTI-1834: the requirement's repository
/// root, not the directory `multi check` was scanned from), which a
/// non-recording fake can't observe.
#[cfg(test)]
#[derive(Default)]
pub struct RecordingSandbox {
    sources: std::sync::Mutex<Vec<PathBuf>>,
}

#[cfg(test)]
impl RecordingSandbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every source path `create` was called with, in call order.
    pub fn sources(&self) -> Vec<PathBuf> {
        self.sources.lock().unwrap().clone()
    }
}

#[cfg(test)]
#[async_trait]
impl Sandbox for RecordingSandbox {
    async fn create(&self, source: &Path) -> Result<SandboxHandle> {
        self.sources.lock().unwrap().push(source.to_path_buf());
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

        let sandbox = select_sandbox(true);
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

    /// Without `--sandbox`, nothing is cloned: the handle is the source itself.
    #[tokio::test]
    async fn disabled_sandbox_hands_back_the_source() {
        let src = tempfile::TempDir::new().unwrap();
        let handle = select_sandbox(false).create(src.path()).await.unwrap();
        assert_eq!(handle.path(), src.path());
        drop(handle);
        assert!(src.path().exists());
    }

    /// MULTI-1818 acceptance: a lease that is never acquired triggers zero
    /// `Sandbox::create` calls — the clone is lazy, not eager.
    #[tokio::test]
    async fn lease_creates_nothing_until_acquired() {
        let sandbox = Arc::new(RecordingSandbox::new());
        let lease = SandboxLease::new(sandbox.clone(), PathBuf::from("/tmp/does-not-matter"));
        drop(lease);
        assert!(sandbox.sources().is_empty());
    }

    /// MULTI-1818: `acquire` clones on its first call and reuses that clone on
    /// any later call from the same lease — the "one sandbox per attempt"
    /// guarantee holds even if a future caller acquires more than once.
    #[tokio::test]
    async fn lease_acquire_clones_once_and_reuses_it() {
        let sandbox = Arc::new(RecordingSandbox::new());
        let source = PathBuf::from("/tmp/does-not-matter");
        let lease = SandboxLease::new(sandbox.clone(), source.clone());

        let first = lease.acquire().await.unwrap().to_path_buf();
        let second = lease.acquire().await.unwrap().to_path_buf();

        assert_eq!(first, second);
        assert_eq!(sandbox.sources(), vec![source]);
    }
}
