//! The discovery phase: find `CHECKS.md` files, parse each to an AST, extract
//! the requirement/check model (including anonymous checks), validate, and
//! assemble the final `Vec<Requirement>` consumed by execution.
//!
//! File loading + parsing run in parallel (one blocking task per file); this
//! module joins the results in deterministic (sorted-file) order and aggregates
//! all validation errors into a single diagnostic rather than failing fast, for
//! authoring ergonomics.

mod parse;
mod walk;

use std::path::Path;

use miette::{IntoDiagnostic, Result};
use multi_core::ManyError;

use crate::checks::model::Requirement;

/// Run the full discovery phase rooted at `root`.
///
/// Returns the validated requirement set (every requirement guaranteed to have
/// ≥1 check), or an aggregated diagnostic naming every offending file/line. An
/// empty tree yields an empty set (the pipeline then succeeds with exit 0).
pub async fn discover(root: &Path) -> Result<Vec<Requirement>> {
    let files = walk::find_checks_files(root)?;

    // Parse + extract each file in parallel on blocking tasks (file IO + CPU).
    let handles: Vec<_> = files
        .into_iter()
        .map(|path| tokio::task::spawn_blocking(move || parse::extract_file(&path)))
        .collect();

    let mut requirements = Vec::new();
    let mut errors = ManyError::default();
    for handle in handles {
        let extraction = handle.await.into_diagnostic()?;
        for err in extraction.errors {
            errors.append(err);
        }
        requirements.extend(extraction.requirements);
    }

    if !errors.is_empty() {
        return Err(errors.into());
    }
    Ok(requirements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn valid_multi_file_tree_returns_all_requirements() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("CHECKS.md"), "# Requirement A\ndo a\n").unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(
            dir.path().join("sub/CHECKS.md"),
            "# Requirement B\n## Check B1\nb1\n## Check B2\nb2\n",
        )
        .unwrap();

        let reqs = discover(dir.path()).await.unwrap();
        assert_eq!(reqs.len(), 2);
        // Each requirement has at least one check.
        assert!(reqs.iter().all(|r| !r.checks.is_empty()));
        // Sorted-file order: root CHECKS.md ("A") before sub/CHECKS.md ("B").
        assert_eq!(reqs[0].title, "A");
        assert_eq!(reqs[1].checks.len(), 2);
    }

    #[tokio::test]
    async fn empty_tree_is_ok_and_empty() {
        let dir = TempDir::new().unwrap();
        let reqs = discover(dir.path()).await.unwrap();
        assert!(reqs.is_empty());
    }

    #[tokio::test]
    async fn orphan_check_aborts_discovery() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("CHECKS.md"),
            "## Check Orphan\nno req above\n",
        )
        .unwrap();
        let err = discover(dir.path()).await.unwrap_err();
        assert!(format!("{err:?}").contains("orphan"));
    }
}
