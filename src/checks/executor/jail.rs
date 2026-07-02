//! Confining a check agent's read-only tools to its sandbox (F4 of the
//! 2026-07-01 timeout postmortem).
//!
//! The CoW sandbox isolates *writes* by construction, but cersei's read-only
//! tools accept absolute paths and will read or walk anywhere the user can.
//! That broke two ways in practice: lost agents launched unbounded recursive
//! globs over the host filesystem (`**/*.rs` over `~/workspace`, even `/`) and
//! timed out inside them, and two agents graded the *live* repository instead
//! of the sandbox copy. [`Jailed`] wraps a tool and rejects any path-bearing
//! input that resolves outside the agent's working directory, returning a tool
//! error that redirects the agent back into the sandbox instead of letting the
//! call run away.

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use cersei_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use serde_json::Value;

/// A [`Tool`] decorator that confines path-bearing inputs to the agent's
/// working directory (read from the [`ToolContext`] at call time, so one
/// wrapper is correct for every concurrently running agent).
pub struct Jailed<T> {
    inner: T,
    /// Input keys that carry a filesystem path (absolute or relative).
    path_keys: &'static [&'static str],
    /// Input key that carries a *glob pattern*, which needs its own rule: the
    /// glob primitive joins the pattern onto its base directory, so an
    /// absolute pattern replaces the base entirely and a `..` component climbs
    /// out of it.
    glob_key: Option<&'static str>,
}

impl<T: Tool> Jailed<T> {
    /// Confine the string inputs named by `path_keys` to the working directory.
    pub fn path_keys(inner: T, path_keys: &'static [&'static str]) -> Self {
        Self {
            inner,
            path_keys,
            glob_key: None,
        }
    }

    /// Like [`Jailed::path_keys`], additionally confining the glob pattern
    /// under `glob_key`.
    pub fn glob(inner: T, path_keys: &'static [&'static str], glob_key: &'static str) -> Self {
        Self {
            inner,
            path_keys,
            glob_key: Some(glob_key),
        }
    }
}

#[async_trait]
impl<T: Tool> Tool for Jailed<T> {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn description(&self) -> &str {
        self.inner.description()
    }
    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }
    fn permission_level(&self) -> PermissionLevel {
        self.inner.permission_level()
    }
    fn category(&self) -> ToolCategory {
        self.inner.category()
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        if let Err(denied) = enforce(&input, &ctx.working_dir, self.path_keys, self.glob_key) {
            return ToolResult::error(denied);
        }
        self.inner.execute(input, ctx).await
    }
}

/// Check every confined input key; `Err` carries the agent-facing denial.
/// Non-object input is left for the inner tool to reject as it sees fit.
fn enforce(
    input: &Value,
    root: &Path,
    path_keys: &[&str],
    glob_key: Option<&str>,
) -> Result<(), String> {
    let Some(obj) = input.as_object() else {
        return Ok(());
    };

    for key in path_keys {
        if let Some(raw) = obj.get(*key).and_then(Value::as_str)
            && !within_root(Path::new(raw), root)
        {
            return Err(deny(Path::new(raw), root));
        }
    }

    if let Some(key) = glob_key
        && let Some(pattern) = obj.get(key).and_then(Value::as_str)
    {
        if Path::new(pattern)
            .components()
            .any(|c| c == Component::ParentDir)
        {
            return Err(format!(
                "glob pattern `{pattern}` contains `..`, which would escape the sandbox \
working directory `{}`. Use a pattern relative to that directory.",
                root.display(),
            ));
        }
        if Path::new(pattern).is_absolute() && !within_root(&literal_prefix(pattern), root) {
            return Err(deny(Path::new(pattern), root));
        }
    }

    Ok(())
}

fn deny(offending: &Path, root: &Path) -> String {
    format!(
        "`{}` is outside the sandbox working directory `{}`. Every project file for this \
check lives under that directory; use a path inside it (or omit `path` to default to it).",
        offending.display(),
        root.display(),
    )
}

/// Whether `candidate` (absolute, or relative to `root`) stays inside `root`.
///
/// The candidate is judged by its *resolved* location, so a symlink inside the
/// sandbox cannot point a tool outside it. Both spellings of the root — as
/// configured and fully resolved — are accepted: macOS temp sandboxes are
/// reached through the `/var` → `/private/var` symlink, so the agent may
/// legitimately hold either form.
fn within_root(candidate: &Path, root: &Path) -> bool {
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let resolved = resolve_lenient(&lexical_normalize(&absolute));

    let mut roots = vec![lexical_normalize(root)];
    if let Ok(resolved_root) = root.canonicalize()
        && !roots.contains(&resolved_root)
    {
        roots.push(resolved_root);
    }

    roots.iter().any(|r| resolved.starts_with(r))
}

/// Canonicalize the deepest existing ancestor of `path` and re-append the
/// remaining (nonexistent) components. Plain `canonicalize` fails on paths
/// that don't exist yet, but their symlink-resolved location is exactly what
/// the jail must judge — e.g. `<sandbox>/escape-link/nope.txt` must be denied,
/// and `/var/<sandbox>/nope.rs` must match a `/private/var/…` root. The input
/// is already lexically normalized, so re-appending cannot reintroduce `..`.
fn resolve_lenient(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match existing.canonicalize() {
            Ok(mut resolved) => {
                for part in tail.iter().rev() {
                    resolved.push(part);
                }
                return resolved;
            }
            Err(_) => match (existing.file_name(), existing.parent()) {
                (Some(name), Some(parent)) => {
                    tail.push(name.to_os_string());
                    existing = parent.to_path_buf();
                }
                // Ran out of components without finding an existing ancestor
                // (or the path ends in `..`/root): judge it as-is.
                _ => return path.to_path_buf(),
            },
        }
    }
}

/// Resolve `.` and `..` components without touching the filesystem.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The leading components of a glob pattern before the first one containing a
/// metacharacter — the directory the walk actually starts from.
fn literal_prefix(pattern: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for component in Path::new(pattern).components() {
        if component
            .as_os_str()
            .to_string_lossy()
            .contains(['*', '?', '[', '{'])
        {
            break;
        }
        out.push(component.as_os_str());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cersei_tools::permissions::AllowReadOnly;
    use cersei_tools::{CostTracker, Extensions, glob_tool::GlobTool};
    use std::sync::Arc;

    fn ctx_in(dir: &Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "jail-test".into(),
            permissions: Arc::new(AllowReadOnly),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    #[test]
    fn relative_paths_stay_inside() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(within_root(Path::new("src/main.rs"), tmp.path()));
        assert!(within_root(Path::new("."), tmp.path()));
    }

    #[test]
    fn absolute_paths_inside_are_allowed_and_outside_denied() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(within_root(&tmp.path().join("src"), tmp.path()));
        assert!(!within_root(Path::new("/etc"), tmp.path()));
        assert!(!within_root(Path::new("/"), tmp.path()));
    }

    #[test]
    fn dotdot_cannot_climb_out() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!within_root(Path::new(".."), tmp.path()));
        assert!(!within_root(&tmp.path().join("../sibling"), tmp.path()));
        // Climbing out and back in is fine — only the resolved location matters.
        let back_in = tmp.path().join("..").join(
            tmp.path()
                .file_name()
                .expect("tempdir has a terminal component"),
        );
        assert!(within_root(&back_in, tmp.path()));
    }

    #[test]
    fn symlink_aliased_roots_are_equivalent() {
        // macOS tempdirs live under `/var/folders`, a symlink into
        // `/private/var/folders`: the raw and canonical spellings must both be
        // accepted, in both directions.
        let tmp = tempfile::tempdir().unwrap();
        let raw = tmp.path().to_path_buf();
        let canonical = raw.canonicalize().unwrap();
        assert!(within_root(&canonical.join("file.rs"), &raw));
        assert!(within_root(&raw.join("file.rs"), &canonical));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_pointing_out_of_the_sandbox_is_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("escape")).unwrap();
        assert!(!within_root(
            &tmp.path().join("escape/secret.txt"),
            tmp.path()
        ));
        // Even a nonexistent path routed through the link resolves outside.
        assert!(!within_root(
            &tmp.path().join("escape/nope.txt"),
            tmp.path()
        ));
    }

    #[test]
    fn glob_patterns_are_confined() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ok = |v: Value| enforce(&v, root, &["path"], Some("pattern")).is_ok();

        assert!(ok(serde_json::json!({ "pattern": "**/*.rs" })));
        assert!(ok(serde_json::json!({ "pattern": "src/**" })));
        // Absolute pattern inside the sandbox names the same walk — allowed.
        assert!(ok(serde_json::json!({
            "pattern": format!("{}/**/*.rs", root.display())
        })));
        // An absolute pattern replaces the base directory: outside is denied.
        assert!(!ok(serde_json::json!({ "pattern": "/Users/**/*.rs" })));
        // `..` climbs out of the base directory before the walk starts.
        assert!(!ok(serde_json::json!({ "pattern": "../**/*.rs" })));
        // The `path` key is confined like any other path input.
        assert!(!ok(serde_json::json!({ "pattern": "*", "path": "/" })));
    }

    #[tokio::test]
    async fn jailed_glob_denies_escapes_with_a_redirecting_error() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = Jailed::glob(GlobTool, &["path"], "pattern");
        let result = tool
            .execute(
                serde_json::json!({ "pattern": "**/*.rs", "path": "/Users" }),
                &ctx_in(tmp.path()),
            )
            .await;
        assert!(result.is_error);
        assert!(
            result
                .content
                .contains("outside the sandbox working directory")
        );
        assert!(result.content.contains(&tmp.path().display().to_string()));
    }

    #[tokio::test]
    async fn jailed_glob_delegates_in_sandbox_calls() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("lib.rs"), "").unwrap();
        let tool = Jailed::glob(GlobTool, &["path"], "pattern");
        let result = tool
            .execute(
                serde_json::json!({ "pattern": "*.rs" }),
                &ctx_in(tmp.path()),
            )
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("lib.rs"));
    }
}
