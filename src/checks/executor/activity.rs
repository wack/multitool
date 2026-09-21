//! Root-relative rendering of an allowlisted tool's primary argument, for the
//! live progress the presenter shows while a check's agent is running
//! (MULTI-1828).
//!
//! Kept in its own module rather than inlined into [`CerseiExecutor`]'s
//! `on_event` hook: a later ticket on a different stack (MULTI-1817) also
//! touches that hook to capture tool calls for the Jev decision engine's
//! evidence replay, and isolating the rendering logic here keeps the hook a
//! thin passthrough — minimizing the eventual merge conflict.
//!
//! [`CerseiExecutor`]: super::cersei::CerseiExecutor
//!
//! The presenter is a display surface, not a trust boundary — [`Jailed`]
//! ([`super::jail`]) is what actually confines a check's tools to its
//! sandbox — but [`render_activity`] still never echoes an absolute path
//! back: the `ToolStart` event it renders from fires on the *raw*
//! model-issued input, before `Jailed` gets a chance to reject it, so a
//! confused or adversarial agent's out-of-sandbox path argument must not
//! leak the sandbox's throwaway host location (or any other absolute host
//! path) into what the user sees.
//!
//! [`Jailed`]: super::jail::Jailed

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// Rendered in place of a path argument that doesn't resolve inside the
/// sandbox root. In practice [`Jailed`](super::jail::Jailed) rejects such a
/// tool call before it runs, but this module renders from the raw
/// `ToolStart` event, which fires before that rejection — so this fallback
/// must stay reachable, and must never be the raw absolute path itself.
const OUTSIDE_SANDBOX: &str = "<outside sandbox>";

/// The allowlisted tools' names, exactly as `cersei_tools` reports them via
/// `Tool::name()` — see [`super::cersei::read_only_tools`]. Any other tool
/// name (crucially, the judge tool, [`super::judge::JUDGE_TOOL`]) renders no
/// activity at all.
const READ: &str = "Read";
const GREP: &str = "Grep";
const GLOB: &str = "Glob";

/// Render a short, root-relative description of an allowlisted `ToolStart`
/// call's primary argument for the presenter, e.g. `Read src/auth/sign.rs`
/// or `Grep "sign_jwt"`.
///
/// Returns `None` when `name` isn't one of the three allowlisted tools, or
/// when the tool's primary argument is missing or not a string — a
/// malformed or unexpected call simply produces no progress update rather
/// than a misleading or panicking one.
pub fn render_activity(name: &str, input: &Value, sandbox_root: &Path) -> Option<String> {
    match name {
        READ => {
            let file_path = input.get("file_path")?.as_str()?;
            Some(format!("Read {}", render_path(file_path, sandbox_root)))
        }
        // Grep/Glob's primary argument is the search/glob pattern, not a
        // path — it names no filesystem location, so it is rendered as a
        // quoted string rather than run through `render_path`.
        GREP => {
            let pattern = input.get("pattern")?.as_str()?;
            Some(format!("Grep {pattern:?}"))
        }
        GLOB => {
            let pattern = input.get("pattern")?.as_str()?;
            Some(format!("Glob {pattern:?}"))
        }
        _ => None,
    }
}

/// Render `raw` — a `file_path` argument value exactly as the agent supplied
/// it — relative to `sandbox_root`: an already-relative path is returned
/// as-is (it can't name anything outside the working directory it would be
/// resolved against); an absolute one renders as `.` when it names the root
/// itself, a relative path when it names something under the root, and
/// [`OUTSIDE_SANDBOX`] when it names anything else.
fn render_path(raw: &str, sandbox_root: &Path) -> String {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return raw.to_string();
    }

    let candidate = lexical_normalize(path);
    for root in root_spellings(sandbox_root) {
        if let Ok(rel) = candidate.strip_prefix(&root) {
            return if rel.as_os_str().is_empty() {
                ".".to_string()
            } else {
                rel.display().to_string()
            };
        }
    }
    OUTSIDE_SANDBOX.to_string()
}

/// Every spelling of `root` worth comparing a tool input against. Always
/// includes `root` itself and, when it exists on disk, its canonicalized
/// form — a macOS temp sandbox is created under `/var/…`, a symlink to
/// `/private/var/…`, and an agent that canonicalizes a path itself (or
/// echoes one back from an earlier tool's already-canonical result) may use
/// either spelling regardless of which one `root` happens to be.
fn root_spellings(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![lexical_normalize(root)];
    if let Ok(resolved) = root.canonicalize() {
        push_unique(&mut roots, resolved);
    }
    // The canonicalize() above only ever maps the raw spelling to the
    // canonical one. Also cover the reverse — `root` is already canonical
    // (`/private/var/…`) but the agent echoes the shorter, raw form
    // (`/var/…`) — via a fixed literal rewrite rather than a real resolve,
    // since the candidate path this is compared against may not exist yet.
    #[cfg(target_os = "macos")]
    for candidate in roots.clone() {
        if let Ok(rest) = candidate.strip_prefix("/private/var") {
            push_unique(&mut roots, Path::new("/var").join(rest));
        } else if let Ok(rest) = candidate.strip_prefix("/var") {
            push_unique(&mut roots, Path::new("/private/var").join(rest));
        }
    }
    roots
}

fn push_unique(roots: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !roots.contains(&candidate) {
        roots.push(candidate);
    }
}

/// Resolve `.`/`..` components without touching the filesystem, so a
/// lexical escape attempt (`sandbox/../../etc/passwd`) can't masquerade as
/// an in-sandbox path just because its unresolved string happens to start
/// with the sandbox root.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::executor::judge::JUDGE_TOOL;

    fn read_input(file_path: &str) -> Value {
        serde_json::json!({ "file_path": file_path })
    }

    #[test]
    fn read_inside_sandbox_renders_root_relative_path() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = read_input("/tmp/sandbox-xyz/src/auth/sign.rs");
        assert_eq!(
            render_activity(READ, &input, root),
            Some("Read src/auth/sign.rs".to_string())
        );
    }

    #[test]
    fn read_at_sandbox_root_renders_dot() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = read_input("/tmp/sandbox-xyz");
        assert_eq!(
            render_activity(READ, &input, root),
            Some("Read .".to_string())
        );
    }

    #[test]
    fn read_relative_path_is_rendered_as_is() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = read_input("src/main.rs");
        assert_eq!(
            render_activity(READ, &input, root),
            Some("Read src/main.rs".to_string())
        );
    }

    /// MULTI-1828 acceptance: activity strings never contain absolute
    /// sandbox (or other host) paths — an out-of-sandbox path renders a
    /// fixed placeholder rather than echoing the raw absolute path back.
    #[test]
    fn read_outside_sandbox_never_leaks_an_absolute_path() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = read_input("/etc/passwd");
        let rendered = render_activity(READ, &input, root).unwrap();
        assert_eq!(rendered, "Read <outside sandbox>");
        assert!(!rendered.contains("/etc/passwd"));
    }

    /// A lexical `..` escape must resolve to *outside* the sandbox, not be
    /// judged by its unresolved string (which still starts with the root).
    #[test]
    fn dotdot_escape_renders_outside_sandbox_not_a_leaked_path() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = read_input("/tmp/sandbox-xyz/../../etc/passwd");
        let rendered = render_activity(READ, &input, root).unwrap();
        assert_eq!(rendered, "Read <outside sandbox>");
    }

    #[test]
    fn grep_renders_the_pattern_quoted_not_as_a_path() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = serde_json::json!({ "pattern": "sign_jwt", "path": "/tmp/sandbox-xyz" });
        assert_eq!(
            render_activity(GREP, &input, root),
            Some("Grep \"sign_jwt\"".to_string())
        );
    }

    #[test]
    fn glob_renders_the_pattern_quoted() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = serde_json::json!({ "pattern": "**/*.rs" });
        assert_eq!(
            render_activity(GLOB, &input, root),
            Some("Glob \"**/*.rs\"".to_string())
        );
    }

    #[test]
    fn missing_primary_argument_renders_no_activity() {
        let root = Path::new("/tmp/sandbox-xyz");
        assert_eq!(render_activity(READ, &serde_json::json!({}), root), None);
        assert_eq!(render_activity(GREP, &serde_json::json!({}), root), None);
        assert_eq!(render_activity(GLOB, &serde_json::json!({}), root), None);
    }

    #[test]
    fn non_string_primary_argument_renders_no_activity() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = serde_json::json!({ "file_path": 12345 });
        assert_eq!(render_activity(READ, &input, root), None);

        let input = serde_json::json!({ "pattern": ["not", "a", "string"] });
        assert_eq!(render_activity(GREP, &input, root), None);
    }

    /// Only the three allowlisted read-only tools ever produce activity —
    /// crucially, the judge tool must not (MULTI-1828 scope).
    #[test]
    fn unlisted_tools_including_the_judge_tool_render_no_activity() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = serde_json::json!({ "success": true });
        assert_eq!(render_activity(JUDGE_TOOL, &input, root), None);
        assert_eq!(
            render_activity("Bash", &serde_json::json!({ "command": "ls" }), root),
            None
        );
    }

    /// macOS mounts `/var` as a symlink to `/private/var`. An agent may echo
    /// either spelling back regardless of which one the acquired sandbox
    /// root itself is spelled as — both directions must resolve to the same
    /// root-relative rendering.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_var_symlink_spelling_is_tolerated_in_both_directions() {
        let tmp = tempfile::tempdir().unwrap();
        let raw_root = tmp.path().to_path_buf();
        let canonical_root = raw_root.canonicalize().unwrap();
        assert_ne!(
            raw_root, canonical_root,
            "test fixture assumption: macOS temp dirs are reached through a symlink"
        );

        // The sandbox root is acquired in its raw (unresolved) spelling, but
        // the agent echoes back the canonical one.
        let input = read_input(canonical_root.join("src/foo.rs").to_str().unwrap());
        assert_eq!(
            render_activity(READ, &input, &raw_root),
            Some("Read src/foo.rs".to_string())
        );

        // And the reverse: the sandbox root is already canonical, but the
        // agent echoes the shorter, raw spelling.
        let input = read_input(raw_root.join("src/foo.rs").to_str().unwrap());
        assert_eq!(
            render_activity(READ, &input, &canonical_root),
            Some("Read src/foo.rs".to_string())
        );
    }
}
