//! macOS APFS copy-on-write sandbox via `clonefile(2)`.
//!
//! `clonefile` produces a near-instant, space-efficient (metadata-only) clone of
//! a whole tree on APFS. The clone lives under a fresh temp directory and is
//! removed when the [`SandboxHandle`] is dropped.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use async_trait::async_trait;
use miette::{IntoDiagnostic, Result, miette};

use super::{Sandbox, SandboxHandle};

/// APFS `clonefile`-backed sandbox.
pub struct ApfsSandbox;

impl ApfsSandbox {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Sandbox for ApfsSandbox {
    async fn create(&self, source: &Path) -> Result<SandboxHandle> {
        let source = source.to_path_buf();
        // `clonefile` is a blocking syscall — run it off the async runtime.
        tokio::task::spawn_blocking(move || clone_tree(&source))
            .await
            .into_diagnostic()?
    }
}

fn clone_tree(source: &Path) -> Result<SandboxHandle> {
    let temp = tempfile::Builder::new()
        .prefix("multi-check-")
        .tempdir()
        .into_diagnostic()?;
    // `clonefile` requires the destination NOT to exist and its parent to exist.
    let dest = temp.path().join("sandbox");

    let src_c = cstring(source)?;
    let dst_c = cstring(&dest)?;

    // SAFETY: both pointers are valid NUL-terminated C strings that outlive the
    // call; `clonefile` does not retain them.
    let ret = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        return Err(miette!(
            "clonefile({}, {}) failed: {err}. APFS copy-on-write requires the source and destination to be on the same APFS volume.",
            source.display(),
            dest.display(),
        ));
    }

    Ok(SandboxHandle {
        root: dest,
        _temp: Some(temp),
    })
}

fn cstring(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|e| miette!("path {} contains an interior NUL byte: {e}", path.display()))
}
