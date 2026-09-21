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

/// The cap on a rendered activity string's length (in `char`s), applied
/// after root-stripping and before the tool-name prefix/quoting is added.
/// Chosen to comfortably fit one inline-TUI row alongside the check title,
/// turn counter, and elapsed timer, while still bounding an otherwise
/// unbounded agent-supplied path or pattern (MULTI-1828 code review).
const MAX_ACTIVITY_LEN: usize = 80;

/// Render a short, root-relative description of an allowlisted `ToolStart`
/// call's primary argument for the presenter, e.g. `Read src/auth/sign.rs`
/// or `Grep "sign_jwt"`.
///
/// Returns `None` when `name` isn't one of the three allowlisted tools, or
/// when the tool's primary argument is missing or not a string — a
/// malformed or unexpected call simply produces no progress update rather
/// than a misleading or panicking one.
///
/// Every primary argument — including Grep/Glob's pattern, which is
/// frequently absolute-path-shaped, or has an absolute path *embedded* in
/// it (a glob pattern like `/private/var/.../sandbox-xyz/src/**/*.rs`, an
/// alternation like `{/tmp/sandbox-xyz/a,/tmp/sandbox-xyz/b}/*.rs`, or a
/// search pattern like `foo|/tmp/sandbox-xyz/src/x`) — is run through the
/// same two-stage sanitizer before it's handed to the sink: [`derelativize`]
/// first (no spelling of the sandbox root survives anywhere in the result,
/// whole-string or embedded), then [`sanitize_for_display`] (no control
/// characters, bounded length). A plain string with no trace of the sandbox
/// root anywhere in it (the common case for both a `Read` path and a
/// `Grep`/`Glob` pattern) passes through both unchanged.
pub fn render_activity(name: &str, input: &Value, sandbox_root: &Path) -> Option<String> {
    match name {
        READ => {
            let file_path = input.get("file_path")?.as_str()?;
            let rendered = sanitize_for_display(&derelativize(file_path, sandbox_root));
            Some(format!("Read {rendered}"))
        }
        GREP => {
            let pattern = input.get("pattern")?.as_str()?;
            let rendered = sanitize_for_display(&derelativize(pattern, sandbox_root));
            Some(format!("Grep {rendered:?}"))
        }
        GLOB => {
            let pattern = input.get("pattern")?.as_str()?;
            let rendered = sanitize_for_display(&derelativize(pattern, sandbox_root));
            Some(format!("Glob {rendered:?}"))
        }
        _ => None,
    }
}

/// Render `raw` — a tool's primary argument exactly as the agent supplied
/// it (a `Read` `file_path`, or a `Grep`/`Glob` `pattern`, which is
/// frequently absolute-path-shaped even though it's a pattern rather than a
/// path) — relative to `sandbox_root`, guaranteeing no spelling of the root
/// survives anywhere in the result.
///
/// Two passes:
/// 1. Whole-string handling. An already-relative string is left as the
///    starting point for pass 2: it can't name anything outside the working
///    directory it would be resolved against, and — for a pattern like
///    `**/*.rs` or a search term like `sign_jwt` — usually isn't a path at
///    all. An absolute one renders as `.` when it names the root itself, a
///    relative remainder when it names something under the root, and
///    [`OUTSIDE_SANDBOX`] when it names anything else.
/// 2. [`scrub_embedded_root`]. Pass 1 only ever judges the string *as a
///    whole* (`Path::is_absolute` / `strip_prefix`), so it does nothing for
///    a string that isn't absolute from its very first character but still
///    has the sandbox root embedded partway through — a `Grep` pattern like
///    `foo|/private/var/.../sandbox-xyz/src/x`, or a `Glob` alternation
///    `{/tmp/sandbox-xyz/a,/tmp/sandbox-xyz/b}/*.rs` (MULTI-1828 code
///    review). Run unconditionally, after pass 1, on whatever pass 1
///    produced — including the placeholder/relative outputs, where it is
///    simply a no-op.
///
/// `Path`/`PathBuf` here are used purely as a slash-splitting string
/// primitive, not a claim that `raw` is a filesystem path: a glob pattern's
/// metacharacters (`*`, `?`, `[`, `{`) are opaque to `Component` parsing —
/// they ride along as ordinary path components — so stripping the root off
/// `/tmp/sandbox-xyz/**/*.rs` correctly yields `**/*.rs`.
fn derelativize(raw: &str, sandbox_root: &Path) -> String {
    let path = Path::new(raw);
    let whole = if path.is_absolute() {
        let candidate = lexical_normalize(path);
        root_spellings(sandbox_root)
            .into_iter()
            .find_map(|root| {
                candidate.strip_prefix(&root).ok().map(|rel| {
                    if rel.as_os_str().is_empty() {
                        ".".to_string()
                    } else {
                        rel.display().to_string()
                    }
                })
            })
            .unwrap_or_else(|| OUTSIDE_SANDBOX.to_string())
    } else {
        raw.to_string()
    };
    scrub_embedded_root(&whole, sandbox_root)
}

/// Scrub every occurrence, anywhere in `text`, of every known spelling of
/// `sandbox_root` (MULTI-1828 code review) — the pass [`derelativize`]'s
/// whole-string handling can't cover, since a string that isn't absolute
/// *as a whole* is never even inspected there, even though the root can
/// still be embedded partway through it (a `Grep` alternation, a `Glob`
/// brace expansion, or free text around an absolute path).
///
/// Spellings are scrubbed longest-first (by rendered length) so a shorter
/// spelling that is a textual prefix of a longer one — `/var/…` of
/// `/private/var/…` — can't half-eat it and leave a mangled `/private`
/// behind. For each spelling: `<root>/` is replaced with nothing (leaving
/// the root-relative remainder in place), then any bare `<root>` left over
/// (no trailing separator — the root named with nothing after it) is
/// replaced with `.`.
fn scrub_embedded_root(text: &str, sandbox_root: &Path) -> String {
    let mut spellings: Vec<String> = root_spellings(sandbox_root)
        .iter()
        .map(|root| root.display().to_string())
        .collect();
    spellings.sort_by_key(|s| std::cmp::Reverse(s.len()));

    let mut scrubbed = text.to_string();
    for root in spellings {
        let root_with_sep = format!("{root}/");
        scrubbed = scrubbed.replace(&root_with_sep, "");
        scrubbed = scrubbed.replace(&root, ".");
    }
    scrubbed
}

/// The last-mile sanitizer every rendered primary argument passes through
/// (MULTI-1828 code review): strip control characters (a raw newline or
/// escape sequence in an agent-supplied path/pattern could otherwise corrupt
/// a TUI row) and cap the length, truncating on a `char` boundary with a
/// trailing ellipsis so an unbounded input can't grow a row without limit.
fn sanitize_for_display(text: &str) -> String {
    let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
    truncate_with_ellipsis(&cleaned, MAX_ACTIVITY_LEN)
}

/// Truncate `text` to at most `max_chars` `char`s, appending `…` when it
/// had to cut anything. Counts/truncates by `char`, never by byte, so a
/// multi-byte UTF-8 sequence is never split.
fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{kept}…")
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

    /// MULTI-1828 code review blocker: `Glob`'s `pattern` is frequently
    /// absolute-path-shaped and must be root-stripped exactly like a `Read`
    /// path — an absolute pattern under the sandbox root renders relative.
    #[test]
    fn glob_absolute_pattern_inside_root_renders_root_relative() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = serde_json::json!({ "pattern": "/tmp/sandbox-xyz/**/*.rs" });
        assert_eq!(
            render_activity(GLOB, &input, root),
            Some("Glob \"**/*.rs\"".to_string())
        );
    }

    /// MULTI-1828 code review blocker: an absolute `Glob` pattern outside
    /// the sandbox root must never leak the raw absolute path.
    #[test]
    fn glob_absolute_pattern_outside_root_renders_placeholder() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = serde_json::json!({ "pattern": "/etc/**/*.rs" });
        let rendered = render_activity(GLOB, &input, root).unwrap();
        assert_eq!(rendered, "Glob \"<outside sandbox>\"");
        assert!(!rendered.contains("/etc"));
    }

    /// MULTI-1828 code review blocker: a `Grep` pattern that happens to be
    /// absolute-path-shaped gets the same treatment as a `Read` path or
    /// `Glob` pattern, not left to leak the sandbox root verbatim.
    #[test]
    fn grep_absolute_path_like_pattern_is_sanitized() {
        let root = Path::new("/tmp/sandbox-xyz");
        let inside = serde_json::json!({ "pattern": "/tmp/sandbox-xyz/secret.txt" });
        assert_eq!(
            render_activity(GREP, &inside, root),
            Some("Grep \"secret.txt\"".to_string())
        );

        let outside = serde_json::json!({ "pattern": "/etc/shadow" });
        let rendered = render_activity(GREP, &outside, root).unwrap();
        assert_eq!(rendered, "Grep \"<outside sandbox>\"");
    }

    /// MULTI-1828 code review hole: the sandbox root can be *embedded*
    /// mid-string in a `Grep` pattern that isn't absolute as a whole (an
    /// alternation like `foo|/tmp/sandbox-xyz/src/x`) — `derelativize`'s
    /// whole-string handling never even inspects such a string, so only the
    /// embedded scrub catches it.
    #[test]
    fn grep_pattern_with_embedded_root_mid_string_is_scrubbed() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = serde_json::json!({ "pattern": "foo|/tmp/sandbox-xyz/src/x" });
        assert_eq!(
            render_activity(GREP, &input, root),
            Some("Grep \"foo|src/x\"".to_string())
        );
    }

    /// MULTI-1828 code review hole: every occurrence of an embedded root is
    /// scrubbed, not just the first — a `Glob` brace alternation like
    /// `{/tmp/sandbox-xyz/a,/tmp/sandbox-xyz/b}/*.rs` embeds the root twice.
    #[test]
    fn glob_pattern_with_multiple_embedded_roots_is_scrubbed() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input =
            serde_json::json!({ "pattern": "{/tmp/sandbox-xyz/a,/tmp/sandbox-xyz/b}/*.rs" });
        assert_eq!(
            render_activity(GLOB, &input, root),
            Some("Glob \"{a,b}/*.rs\"".to_string())
        );
    }

    /// MULTI-1828 code review hole: a bare embedded occurrence of the
    /// sandbox root — no trailing separator, nothing after it — renders as
    /// `.`, exactly like a whole-string match at the root.
    #[test]
    fn grep_pattern_with_bare_embedded_root_renders_dot() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = serde_json::json!({ "pattern": "found in /tmp/sandbox-xyz" });
        assert_eq!(
            render_activity(GREP, &input, root),
            Some("Grep \"found in .\"".to_string())
        );
    }

    /// MULTI-1828 code review hole: spellings are scrubbed longest-first so
    /// the shorter `/var/…` alias can't half-eat an embedded longer
    /// `/private/var/…` spelling and leave a mangled `/private` behind.
    #[cfg(target_os = "macos")]
    #[test]
    fn embedded_root_scrub_prefers_the_longest_spelling_first() {
        let tmp = tempfile::tempdir().unwrap();
        let raw_root = tmp.path().to_path_buf();
        let canonical_root = raw_root.canonicalize().unwrap();
        assert_ne!(
            raw_root, canonical_root,
            "test fixture assumption: macOS temp dirs are reached through a symlink"
        );

        // The sandbox root is acquired in its raw spelling, but the agent's
        // pattern embeds the longer, canonical spelling mid-string.
        let embedded = format!("foo|{}/x", canonical_root.display());
        let input = serde_json::json!({ "pattern": embedded });
        let rendered = render_activity(GREP, &input, &raw_root).unwrap();
        assert_eq!(rendered, "Grep \"foo|x\"");
        assert!(!rendered.contains("/private"), "{rendered}");
    }

    /// MULTI-1828 code review blocker (property-style): across a table of
    /// inputs spanning all three tools — relative, absolute-inside-root,
    /// absolute-outside-root, a lexical `..` escape, and (the code-review
    /// hole) a root embedded mid-string or repeated — no rendered activity
    /// ever contains *any* known spelling of the sandbox root anywhere, and
    /// no argument portion ever starts with `/`.
    #[test]
    fn no_rendered_activity_ever_leaks_the_sandbox_root_or_an_absolute_path() {
        let root = Path::new("/tmp/sandbox-xyz");
        let spellings: Vec<String> = root_spellings(root)
            .iter()
            .map(|r| r.display().to_string())
            .collect();
        let cases: Vec<(&str, Value)> = vec![
            (READ, read_input("src/relative.rs")),
            (READ, read_input("/tmp/sandbox-xyz/src/main.rs")),
            (READ, read_input("/tmp/sandbox-xyz")),
            (READ, read_input("/etc/passwd")),
            (READ, read_input("/tmp/sandbox-xyz/../../etc/passwd")),
            // Pathological nested duplicate: the root appears twice in one
            // absolute path.
            (READ, read_input("/tmp/sandbox-xyz/a/tmp/sandbox-xyz/b")),
            (GREP, serde_json::json!({ "pattern": "sign_jwt" })),
            (
                GREP,
                serde_json::json!({ "pattern": "/tmp/sandbox-xyz/secret" }),
            ),
            (GREP, serde_json::json!({ "pattern": "/etc/shadow" })),
            // Mid-string / multi-occurrence embedded roots — the code-review
            // hole: none of these are absolute *as a whole*.
            (
                GREP,
                serde_json::json!({ "pattern": "foo|/tmp/sandbox-xyz/src/x" }),
            ),
            (
                GREP,
                serde_json::json!({ "pattern": "found in /tmp/sandbox-xyz" }),
            ),
            (GLOB, serde_json::json!({ "pattern": "**/*.rs" })),
            (
                GLOB,
                serde_json::json!({ "pattern": "/tmp/sandbox-xyz/**/*.rs" }),
            ),
            (GLOB, serde_json::json!({ "pattern": "/var/**/*.rs" })),
            (
                GLOB,
                serde_json::json!({ "pattern": "{/tmp/sandbox-xyz/a,/tmp/sandbox-xyz/b}/*.rs" }),
            ),
        ];

        for (name, input) in cases {
            let rendered = render_activity(name, &input, root).unwrap();
            for spelling in &spellings {
                assert!(
                    !rendered.contains(spelling.as_str()),
                    "leaked sandbox-root spelling {spelling:?} in {rendered:?}"
                );
            }
            let (_, arg) = rendered
                .split_once(' ')
                .expect("every rendered activity has a tool-name prefix");
            let arg = arg.trim_matches('"');
            assert!(
                !arg.starts_with('/'),
                "argument portion leaked an absolute path: {rendered:?}"
            );
        }
    }

    /// MULTI-1828 code review minor: a raw newline (or other control
    /// character) in an agent-supplied path/pattern must not survive into
    /// the rendered activity — it would corrupt a TUI row.
    #[test]
    fn control_characters_are_stripped() {
        let root = Path::new("/tmp/sandbox-xyz");
        let input = read_input("src/evil\n\t\r.rs");
        let rendered = render_activity(READ, &input, root).unwrap();
        assert_eq!(rendered, "Read src/evil.rs");
        assert!(rendered.chars().all(|c| !c.is_control()));
    }

    /// MULTI-1828 code review minor: an unbounded relative path/pattern is
    /// truncated to a bounded length, on a `char` boundary, with a trailing
    /// ellipsis rather than growing the row without limit.
    #[test]
    fn overlong_activity_is_truncated_with_an_ellipsis() {
        let root = Path::new("/tmp/sandbox-xyz");
        let long_name = "a".repeat(200);
        let input = read_input(&format!("src/{long_name}.rs"));
        let rendered = render_activity(READ, &input, root).unwrap();

        // "Read " (5 chars) + the sanitized/truncated argument.
        assert_eq!(rendered.chars().count(), 5 + MAX_ACTIVITY_LEN);
        assert!(rendered.ends_with('…'), "{rendered}");
        assert!(!rendered.contains(&long_name), "{rendered}");
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
