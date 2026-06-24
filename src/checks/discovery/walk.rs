//! Recursively discover `CHECKS.md` files beneath the working directory.
//!
//! Uses the `ignore` crate (the ripgrep walker) for a fast, gitignore-aware
//! walk. Ignore files (`.gitignore`, etc.) are **respected** by default so that
//! generated / vendored trees are skipped.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use miette::{IntoDiagnostic, Result};

/// The exact filename that declares checks.
pub const CHECKS_FILENAME: &str = "CHECKS.md";

/// Recursively find every file named exactly `CHECKS.md` under `root`.
///
/// Results are sorted lexicographically so downstream parsing and reporting are
/// deterministic regardless of filesystem iteration order.
pub fn find_checks_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in WalkBuilder::new(root).standard_filters(true).build() {
        let entry = entry.into_diagnostic()?;
        let is_file = entry.file_type().is_some_and(|ft| ft.is_file());
        if is_file && entry.file_name() == CHECKS_FILENAME {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn finds_nested_checks_and_ignores_others() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::write(root.join("CHECKS.md"), "# Requirement A\ndo a\n").unwrap();
        fs::create_dir_all(root.join("sub/deep")).unwrap();
        fs::write(root.join("sub/CHECKS.md"), "# Requirement B\ndo b\n").unwrap();
        fs::write(root.join("sub/deep/CHECKS.md"), "# Requirement C\ndo c\n").unwrap();
        // Non-matching files are ignored.
        fs::write(root.join("README.md"), "nope").unwrap();
        fs::write(root.join("sub/checks.md"), "wrong case").unwrap();

        let found = find_checks_files(root).unwrap();
        assert_eq!(found.len(), 3, "found: {found:?}");
        assert!(found.iter().all(|p| p.file_name().unwrap() == "CHECKS.md"));
    }

    #[test]
    fn empty_tree_yields_empty_set() {
        let dir = TempDir::new().unwrap();
        let found = find_checks_files(dir.path()).unwrap();
        assert!(found.is_empty());
    }
}
