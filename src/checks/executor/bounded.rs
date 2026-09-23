//! Refusing search results too large for the agent to see whole.
//!
//! A broad `Grep`/`Glob` (`**/*.rs` over the whole repository) is cut down
//! twice before the agent reasons over it: by the tool's own result cap
//! (`Grep`'s hardcoded 250 matches, `Glob`'s `limit`, default 200) and then by
//! the agent runner, which rewrites any tool result over
//! [`MAX_VISIBLE_LINES`] lines or [`MAX_VISIBLE_CHARS`] characters into a
//! head/tail excerpt. Either way the agent concludes from a partial view — and
//! when such a call lands in a check's frozen plan, the Jev decision engine
//! can't trust it, so `multi plan` marks the whole check agent-only
//! ("truncated discovery").
//!
//! [`Bounded`] wraps a search tool and turns an over-bound result into a tool
//! error that tells the agent how the matches are distributed, so it narrows
//! the search instead. Errored calls are never captured as plan evidence (see
//! `tool_capture`), so a capped result can no longer reach a plan at all.
//!
//! Replay (`checks::jev::replay`) deliberately does **not** use this wrapper:
//! it must see raw tool output to detect a frozen call that has since grown
//! past its tool's cap.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use cersei_tools::{PermissionLevel, Tool, ToolCategory, ToolContext, ToolResult};
use serde_json::Value;

/// The most result lines the agent runner passes through verbatim. Mirrors
/// the pinned `cersei-agent/src/runner.rs`'s `cap_tool_result`, which excerpts
/// any result with more than `MAX_HEAD_LINES + MAX_TAIL_LINES + 5` (80 + 80 +
/// 5) lines.
const MAX_VISIBLE_LINES: usize = 165;

/// The most result characters the agent runner passes through verbatim
/// (`runner.rs`'s `MAX_SINGLE_RESULT_CHARS`), for results with few but very
/// long lines.
const MAX_VISIBLE_CHARS: usize = 20_000;

/// cersei's `GrepTool` hardcoded match cap (`grep_tool.rs`:
/// `max_results: Some(250)`). A result of exactly this many lines may be
/// missing matches, so the reported count is a lower bound.
const GREP_MATCH_CAP: usize = 250;

/// A substring of `GlobTool`'s truncation notice (`glob_tool.rs`), which it
/// appends whenever more paths matched than its effective `limit`.
const GLOB_TRUNCATION_NOTICE: &str = "more exist. Use a more specific pattern to narrow results.";

/// How many files/directories the refusal's distribution summary lists.
const SUMMARY_ENTRIES: usize = 10;

/// Which search tool is wrapped, deciding how its output is parsed.
#[derive(Debug, Clone, Copy)]
enum Search {
    /// `path:line:content` lines, summarized by file.
    Grep,
    /// One path per line, summarized by parent directory.
    Glob,
}

/// A [`Tool`] decorator that refuses search results the agent can't see whole
/// (see the module docs).
pub struct Bounded<T> {
    inner: T,
    search: Search,
}

impl<T: Tool> Bounded<T> {
    /// Bound a `Grep`-shaped tool (`path:line:content` output).
    pub fn grep(inner: T) -> Self {
        Self {
            inner,
            search: Search::Grep,
        }
    }

    /// Bound a `Glob`-shaped tool (one path per line).
    pub fn glob(inner: T) -> Self {
        Self {
            inner,
            search: Search::Glob,
        }
    }
}

#[async_trait]
impl<T: Tool> Tool for Bounded<T> {
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
        let result = self.inner.execute(input, ctx).await;
        if result.is_error {
            return result;
        }
        match refusal(
            self.search,
            self.inner.name(),
            &result.content,
            &ctx.working_dir,
        ) {
            Some(message) => ToolResult::error(message),
            None => result,
        }
    }
}

/// The agent-facing refusal for an over-bound `output`, or `None` when the
/// agent can see it whole.
fn refusal(search: Search, tool: &str, output: &str, root: &Path) -> Option<String> {
    let entries: Vec<&str> = output
        .lines()
        .filter(|line| !line.is_empty() && !line.contains(GLOB_TRUNCATION_NOTICE))
        .collect();
    let capped = match search {
        Search::Grep => entries.len() >= GREP_MATCH_CAP,
        Search::Glob => output.contains(GLOB_TRUNCATION_NOTICE),
    };
    let oversized = output.lines().count() > MAX_VISIBLE_LINES || output.len() > MAX_VISIBLE_CHARS;
    if !capped && !oversized {
        return None;
    }

    let (noun, group) = match search {
        Search::Grep => ("lines", "file"),
        Search::Glob => ("paths", "directory"),
    };
    let count = if capped {
        format!("at least {}", entries.len())
    } else {
        entries.len().to_string()
    };
    let summary = summarize(search, &entries, root);
    Some(format!(
        "`{tool}` matched {count} {noun} — too many to review reliably{capped_note}, so the \
result was withheld. Narrow the search: scope `path` to a subdirectory, pass a more specific \
glob (e.g. `src/checks/**/*.rs` rather than `**/*.rs`), or use a more specific `pattern`. \
Keep each search under {MAX_VISIBLE_LINES} {noun}.\n\
Matches by {group}{partial}:\n{summary}",
        capped_note = if capped {
            " (the tool's result cap was hit, so the list is incomplete)"
        } else {
            ""
        },
        partial = if capped {
            " (among those returned)"
        } else {
            ""
        },
    ))
}

/// The top [`SUMMARY_ENTRIES`] files (`Grep`) or directories (`Glob`) by
/// match count, root-relative, most matches first.
fn summarize(search: Search, entries: &[&str], root: &Path) -> String {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        let key = match search {
            Search::Grep => grep_file(entry).to_string(),
            Search::Glob => Path::new(entry)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        };
        *counts.entry(relative(&key, root)).or_default() += 1;
    }

    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));

    let mut lines: Vec<String> = ranked
        .iter()
        .take(SUMMARY_ENTRIES)
        .map(|(key, n)| format!("  {key}: {n}"))
        .collect();
    if ranked.len() > SUMMARY_ENTRIES {
        lines.push(format!("  … and {} more", ranked.len() - SUMMARY_ENTRIES));
    }
    lines.join("\n")
}

/// The file part of a `path:line:content` grep line: everything before the
/// first `:<digits>:` separator (a bare `:` may appear in the path itself).
fn grep_file(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(offset) = line[i..].find(':') {
        let colon = i + offset;
        let digits = bytes[colon + 1..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits > 0 && bytes.get(colon + 1 + digits) == Some(&b':') {
            return &line[..colon];
        }
        i = colon + 1;
    }
    line
}

/// `path` relative to `root` when it lies under it (the tools echo absolute
/// sandbox paths), `.` for `root` itself, and unchanged otherwise.
fn relative(path: &str, root: &Path) -> String {
    match Path::new(path).strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cersei_tools::permissions::AllowReadOnly;
    use cersei_tools::{CostTracker, Extensions, glob_tool::GlobTool, grep_tool::GrepTool};
    use std::sync::Arc;

    fn ctx_in(dir: &Path) -> ToolContext {
        ToolContext {
            working_dir: dir.to_path_buf(),
            session_id: "bounded-test".into(),
            permissions: Arc::new(AllowReadOnly),
            cost_tracker: Arc::new(CostTracker::new()),
            mcp_manager: None,
            extensions: Extensions::default(),
        }
    }

    /// `files` files under `dir`, each with `lines` lines containing `NEEDLE`.
    fn populate(dir: &Path, files: usize, lines: usize) {
        for f in 0..files {
            let body: String = (0..lines).map(|l| format!("NEEDLE {l}\n")).collect();
            std::fs::write(dir.join(format!("f{f:03}.txt")), body).unwrap();
        }
    }

    #[test]
    fn grep_file_splits_on_the_line_number_separator() {
        assert_eq!(grep_file("/sb/src/a.rs:12:let x = 1;"), "/sb/src/a.rs");
        // A `:` in the content (or path) that isn't `:<digits>:` is skipped.
        assert_eq!(grep_file("/sb/a.rs:3:foo::bar()"), "/sb/a.rs");
        assert_eq!(grep_file("/sb/we:ird/a.rs:3:x"), "/sb/we:ird/a.rs");
    }

    #[test]
    fn small_results_pass_through() {
        let root = Path::new("/sb");
        let output = (0..MAX_VISIBLE_LINES)
            .map(|i| format!("/sb/a.rs:{i}:x"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(refusal(Search::Grep, "Grep", &output, root).is_none());
    }

    #[test]
    fn results_the_runner_would_excerpt_are_refused() {
        let root = Path::new("/sb");
        let output = (0..=MAX_VISIBLE_LINES)
            .map(|i| format!("/sb/src/a.rs:{i}:x"))
            .collect::<Vec<_>>()
            .join("\n");
        let message = refusal(Search::Grep, "Grep", &output, root).unwrap();
        assert!(message.contains("matched 166 lines"), "got: {message}");
        assert!(message.contains("src/a.rs: 166"), "got: {message}");
        assert!(!message.contains("at least"), "got: {message}");
    }

    #[test]
    fn a_few_very_long_lines_are_refused() {
        let root = Path::new("/sb");
        let long = "x".repeat(MAX_VISIBLE_CHARS);
        let output = format!("/sb/a.min.js:1:{long}");
        assert!(refusal(Search::Grep, "Grep", &output, root).is_some());
    }

    #[tokio::test]
    async fn capped_grep_is_refused_with_a_per_file_summary() {
        let tmp = tempfile::tempdir().unwrap();
        populate(tmp.path(), 30, 10);
        let tool = Bounded::grep(GrepTool);
        let result = tool
            .execute(
                serde_json::json!({ "pattern": "NEEDLE", "path": tmp.path().to_str().unwrap() }),
                &ctx_in(tmp.path()),
            )
            .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("at least 250 lines"),
            "got: {}",
            result.content
        );
        assert!(result.content.contains("result cap was hit"));
        // Root-relative file names, top entries only.
        assert!(result.content.contains("f0"), "got: {}", result.content);
        assert!(!result.content.contains(&tmp.path().display().to_string()));
        assert!(result.content.contains("… and"));
    }

    #[tokio::test]
    async fn narrow_grep_is_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        populate(tmp.path(), 30, 10);
        let tool = Bounded::grep(GrepTool);
        let result = tool
            .execute(
                serde_json::json!({ "pattern": "NEEDLE", "glob": "f001.txt", "path": tmp.path().to_str().unwrap() }),
                &ctx_in(tmp.path()),
            )
            .await;
        assert!(!result.is_error, "got: {}", result.content);
        assert_eq!(result.content.lines().count(), 10);
    }

    #[tokio::test]
    async fn truncated_glob_is_refused_even_below_the_line_bound() {
        let tmp = tempfile::tempdir().unwrap();
        populate(tmp.path(), 12, 1);
        let tool = Bounded::glob(GlobTool);
        // An explicit small `limit` truncates well under MAX_VISIBLE_LINES.
        let result = tool
            .execute(
                serde_json::json!({ "pattern": "*.txt", "limit": 5 }),
                &ctx_in(tmp.path()),
            )
            .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("at least 5 paths"),
            "got: {}",
            result.content
        );
        assert!(result.content.contains("Matches by directory"));
        assert!(result.content.contains("  .: 5"), "got: {}", result.content);
    }

    #[tokio::test]
    async fn complete_glob_is_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        populate(tmp.path(), 12, 1);
        let tool = Bounded::glob(GlobTool);
        let result = tool
            .execute(
                serde_json::json!({ "pattern": "*.txt" }),
                &ctx_in(tmp.path()),
            )
            .await;
        assert!(!result.is_error, "got: {}", result.content);
        assert_eq!(result.content.lines().count(), 12);
    }
}
