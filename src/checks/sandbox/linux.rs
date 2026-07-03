//! Linux copy-on-write sandbox via reflinks (the `FICLONE` ioctl).
//!
//! Linux has no whole-tree clone syscall like macOS's `clonefile`, so we
//! recreate the directory structure under a fresh temp directory and clone each
//! regular file with `FICLONE` — a reflink, i.e. a metadata-only copy-on-write
//! share of the file's extents. `FICLONE` is only supported on copy-on-write
//! filesystems (Btrfs, XFS with `reflink=1`, bcachefs, ...); on others (ext4,
//! tmpfs) it fails and we fall back to a plain byte copy, so the sandbox is
//! still an independent clone of the tree — just without the CoW space savings.
//! The clone lives under a temp directory and is removed when the
//! [`SandboxHandle`] is dropped.

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::Path;

use async_trait::async_trait;
use miette::{IntoDiagnostic, Result, miette};

use super::{Sandbox, SandboxHandle};

/// Reflink (`FICLONE`)-backed sandbox with a plain-copy fallback.
#[derive(Default)]
pub struct ReflinkSandbox;

impl ReflinkSandbox {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Sandbox for ReflinkSandbox {
    async fn create(&self, source: &Path) -> Result<SandboxHandle> {
        let source = source.to_path_buf();
        // Cloning a tree is blocking filesystem work — keep it off the runtime.
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
    // Mirror the macOS layout: the clone root is a `sandbox` directory under the
    // temp directory, which owns teardown.
    let dest = temp.path().join("sandbox");

    copy_dir(source, &dest)?;

    Ok(SandboxHandle {
        root: dest,
        _temp: Some(temp),
    })
}

/// Recursively recreate `source` at `dest`, reflinking regular files where the
/// filesystem supports it and copying them otherwise. Directories are recreated
/// and symlinks are copied verbatim (the link, not its target).
fn copy_dir(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).into_diagnostic()?;
    // Carry the directory's permission bits across.
    let perms = fs::metadata(source).into_diagnostic()?.permissions();
    fs::set_permissions(dest, perms).into_diagnostic()?;

    for entry in fs::read_dir(source).into_diagnostic()? {
        let entry = entry.into_diagnostic()?;
        let file_type = entry.file_type().into_diagnostic()?;
        let src_path = entry.path();
        let dst_path = dest.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else if file_type.is_symlink() {
            let target = fs::read_link(&src_path).into_diagnostic()?;
            std::os::unix::fs::symlink(target, &dst_path).into_diagnostic()?;
        } else {
            reflink_or_copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Clone one regular file with a reflink, falling back to a byte copy when the
/// filesystem does not support `FICLONE`.
fn reflink_or_copy(src: &Path, dst: &Path) -> Result<()> {
    if reflink(src, dst).is_ok() {
        return Ok(());
    }
    // Fallback: a plain copy still yields an independent file (just no CoW).
    // `fs::copy` carries the permission bits across as well.
    fs::copy(src, dst).map_err(|err| {
        miette!(
            "copying {} to {} failed: {err}",
            src.display(),
            dst.display()
        )
    })?;
    Ok(())
}

/// Attempt a `FICLONE` reflink of `src` into a freshly created `dst`. Returns an
/// error (leaving no destination file behind) if the filesystem or targets do
/// not support reflinking.
fn reflink(src: &Path, dst: &Path) -> std::io::Result<()> {
    let src_file = File::open(src)?;
    let perms = src_file.metadata()?.permissions();
    let dst_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(perms.mode())
        .open(dst)?;

    // SAFETY: both descriptors are valid and owned for the duration of the call.
    // `FICLONE` links the source fd's extents into the (empty) destination fd and
    // retains neither descriptor.
    let ret = unsafe { libc::ioctl(dst_file.as_raw_fd(), libc::FICLONE, src_file.as_raw_fd()) };
    if ret != 0 {
        let err = std::io::Error::last_os_error();
        // Discard the empty destination we created so the copy fallback can
        // recreate it with `create_new`.
        drop(dst_file);
        let _ = fs::remove_file(dst);
        return Err(err);
    }
    // The open above applied the umask to `mode`; restore the source's exact
    // permission bits so the clone matches (as `fs::copy` does on the fallback).
    dst_file.set_permissions(perms)?;
    Ok(())
}
