//! Resolve a requirement's repository root (MULTI-1834).
//!
//! A requirement's scope is exactly one repository: the *repository root* is
//! the nearest directory, at or above the requirements file, that contains a
//! MultiTool manifest. This is resolved **per requirements file** — starting
//! from the file's own directory, never from the process's current directory
//! or the scan directory `multi check` was invoked with — so a monorepo with
//! one manifest per service scopes each service's requirements to that
//! service, and the result is identical regardless of how the command was
//! invoked. When no manifest exists above the file, the root falls back to
//! the scan directory: exactly the pre-MULTI-1834 behavior, so manifest-less
//! projects keep working unchanged.

use std::path::{Path, PathBuf};

use crate::checks::model::RootSource;

/// Resolve the repository root for a requirements file at `declared_in`,
/// falling back to `scan_root` when no MultiTool manifest exists above it.
///
/// `declared_in` may be relative (`walk::find_checks_files` preserves
/// whatever relativeness the scan directory had) or absolute;
/// [`crate::fs::find_manifest_root`] absolutizes it internally, so either
/// works and the returned root is always absolute. `scan_root`, by contrast,
/// **must already be absolute** — it is used verbatim as the fallback root,
/// and [`discover`](super::discover) resolves it to absolute exactly once
/// before any file reaches this function, so every requirement's root shares
/// one absolute spelling regardless of which branch below is taken.
pub(super) fn resolve(declared_in: &Path, scan_root: &Path) -> (PathBuf, RootSource) {
    debug_assert!(
        scan_root.is_absolute(),
        "scan_root must be resolved to absolute before reaching repo_root::resolve"
    );
    let start = declared_in.parent().unwrap_or(declared_in);
    match crate::fs::find_manifest_root(start) {
        Some(root) => (root, RootSource::Manifest),
        None => (scan_root.to_path_buf(), RootSource::ScanDirectory),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn manifest_found_above_the_file_is_the_root() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("MultiTool.toml"), "").unwrap();
        let nested = dir.path().join("services/keystore");
        fs::create_dir_all(&nested).unwrap();
        let declared_in = nested.join("CHECKS.md");

        let (root, source) = resolve(&declared_in, Path::new("/unused-scan-root"));
        assert_eq!(root, dir.path());
        assert_eq!(source, RootSource::Manifest);
    }

    #[test]
    fn no_manifest_falls_back_to_the_scan_directory() {
        let dir = TempDir::new().unwrap();
        let declared_in = dir.path().join("CHECKS.md");
        let scan_root = dir.path().join("elsewhere-scan-root");

        let (root, source) = resolve(&declared_in, &scan_root);
        assert_eq!(root, scan_root);
        assert_eq!(source, RootSource::ScanDirectory);
    }
}
