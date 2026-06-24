//! Parse a single `CHECKS.md` into a Markdown AST (via `comrak`) and walk it to
//! extract [`Requirement`]s and their [`Check`]s.
//!
//! Sentinel rules (header-encoded metadata):
//!
//! * An `H1` whose text matches `^(Requirement|Req)\s+` declares a **requirement**;
//!   the remainder is the title.
//! * An `H2` whose text matches `^Check\b` declares a **check**, associated with
//!   the nearest preceding requirement. The Markdown beneath it (until the next
//!   sentinel heading) is the check **prompt**.
//! * Anything else is **prose**: optional metadata. When a requirement declares
//!   no explicit `## Check`, its prose body is promoted to a single **anonymous
//!   check** that inherits the requirement's title.
//!
//! Validation errors (orphan check, checkless requirement) are collected rather
//! than thrown so the caller can aggregate them across files.

use std::path::Path;

use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use miette::{Diagnostic, miette};
use thiserror::Error;

use crate::checks::model::{Check, Requirement};

/// The result of extracting one file: the requirements it declared plus any
/// validation errors (each a ready-to-render `miette` diagnostic).
pub struct FileExtraction {
    pub requirements: Vec<Requirement>,
    pub errors: Vec<miette::Error>,
}

#[derive(Debug, Error, Diagnostic)]
#[error("orphan check in {file}: a `## Check` at line {line} has no preceding requirement")]
#[diagnostic(help("move this `## Check` beneath a `# Requirement` / `# Req` heading"))]
struct OrphanCheck {
    file: String,
    line: usize,
}

#[derive(Debug, Error, Diagnostic)]
#[error(
    "checkless requirement in {file}: requirement {title:?} (line {line}) declares no `## Check` and has no prose to infer an anonymous check from"
)]
#[diagnostic(help(
    "add a `## Check ...` beneath it, or write prose describing how to validate it"
))]
struct ChecklessRequirement {
    file: String,
    title: String,
    line: usize,
}

/// A structural sentinel found while scanning the AST.
enum Marker {
    Requirement { title: String, line: usize },
    Check { title: String, line: usize },
}

/// Parse `source` (the contents of `path`) and extract its requirements/checks.
///
/// This is the AST-walking heart of discovery (MULTI-1335/1336/1337/1338). It
/// never fails outright — structural problems are returned as `errors` so the
/// caller can aggregate across all files.
pub fn extract(path: &Path, source: &str) -> FileExtraction {
    let file = path.display().to_string();
    let line_starts = line_byte_starts(source);

    // Parse to a comrak AST. The arena owns the nodes for this scope.
    let arena = Arena::new();
    let root = parse_document(&arena, source, &Options::default());

    // Pass 1: collect the structural markers (Requirement/Check headings) in
    // document order, with the 1-based line each heading starts and ends on.
    let mut markers: Vec<(Marker, usize)> = Vec::new(); // (marker, heading_end_line)
    for node in root.children() {
        let (level, start_line, end_line) = {
            let data = node.data();
            match &data.value {
                NodeValue::Heading(h) => {
                    (h.level, data.sourcepos.start.line, data.sourcepos.end.line)
                }
                _ => continue,
            }
        };
        let text = heading_text(node);
        let marker = match level {
            1 => requirement_title(&text).map(|title| Marker::Requirement {
                title,
                line: start_line,
            }),
            2 => check_title(&text).map(|title| Marker::Check {
                title,
                line: start_line,
            }),
            _ => None,
        };
        if let Some(marker) = marker {
            markers.push((marker, end_line));
        }
    }

    // Pass 2: assemble requirements. Each marker's body is the raw Markdown from
    // the line after its heading up to (but not including) the next marker.
    let total_lines = line_starts.len();
    let mut errors: Vec<miette::Error> = Vec::new();
    let mut builders: Vec<ReqBuilder> = Vec::new();

    for (idx, (marker, heading_end_line)) in markers.iter().enumerate() {
        let next_start = markers
            .get(idx + 1)
            .map(|(m, _)| marker_line(m))
            .unwrap_or(total_lines + 1);
        let body = slice_lines(source, &line_starts, heading_end_line + 1, next_start);

        match marker {
            Marker::Requirement { title, line } => {
                builders.push(ReqBuilder {
                    title: title.clone(),
                    line: *line,
                    prose: body.to_string(),
                    checks: Vec::new(),
                });
            }
            Marker::Check { title, line } => match builders.last_mut() {
                Some(req) => {
                    let title = if title.is_empty() {
                        req.title.clone() // inherit when `## Check` has no title text
                    } else {
                        title.clone()
                    };
                    req.checks.push(Check {
                        title,
                        prompt: body.to_string(),
                    });
                }
                None => errors.push(
                    OrphanCheck {
                        file: file.clone(),
                        line: *line,
                    }
                    .into(),
                ),
            },
        }
    }

    // Pass 3: anonymous-check inference + checkless validation.
    let mut requirements = Vec::new();
    for builder in builders {
        let ReqBuilder {
            title,
            line,
            prose,
            mut checks,
        } = builder;
        if checks.is_empty() {
            let prose = prose.trim();
            if prose.is_empty() {
                errors.push(
                    ChecklessRequirement {
                        file: file.clone(),
                        title: title.clone(),
                        line,
                    }
                    .into(),
                );
                continue;
            }
            // Promote prose to a single anonymous check inheriting the req title.
            checks.push(Check {
                title: title.clone(),
                prompt: prose.to_string(),
            });
        }
        requirements.push(Requirement {
            filepath: path.to_path_buf(),
            title,
            checks,
        });
    }

    FileExtraction {
        requirements,
        errors,
    }
}

/// A requirement under construction during extraction.
struct ReqBuilder {
    title: String,
    line: usize,
    prose: String,
    checks: Vec<Check>,
}

fn marker_line(m: &Marker) -> usize {
    match m {
        Marker::Requirement { line, .. } | Marker::Check { line, .. } => *line,
    }
}

/// If `text` declares a requirement (`Requirement <title>` / `Req <title>`),
/// return the title. Requires whitespace after the keyword, per the spec's
/// `^(Requirement|Req) ` sentinel.
fn requirement_title(text: &str) -> Option<String> {
    for kw in ["Requirement", "Req"] {
        if let Some(rest) = text.strip_prefix(kw)
            && rest.starts_with(char::is_whitespace)
        {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// If `text` declares a check (`Check` or `Check <title>`), return the title
/// (empty string for a bare `## Check`, which inherits the requirement title).
fn check_title(text: &str) -> Option<String> {
    let rest = text.strip_prefix("Check")?;
    if rest.is_empty() {
        return Some(String::new());
    }
    if rest.starts_with(char::is_whitespace) {
        return Some(rest.trim().to_string());
    }
    None
}

/// Concatenate the inline text of a heading node into a plain string.
fn heading_text<'a>(node: &'a comrak::nodes::AstNode<'a>) -> String {
    let mut out = String::new();
    for d in node.descendants() {
        match &d.data().value {
            NodeValue::Text(s) => out.push_str(s.as_ref()),
            NodeValue::Code(c) => out.push_str(&c.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
            _ => {}
        }
    }
    out
}

/// Byte offset of the start of each 1-based line in `source`.
fn line_byte_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Slice the raw source spanning 1-based lines `[start_line, end_line_excl)`,
/// trimmed. Out-of-range bounds clamp to the document.
fn slice_lines<'a>(
    source: &'a str,
    line_starts: &[usize],
    start_line: usize,
    end_line_excl: usize,
) -> &'a str {
    let byte_at = |line_1based: usize| -> usize {
        if line_1based == 0 {
            0
        } else if line_1based - 1 < line_starts.len() {
            line_starts[line_1based - 1]
        } else {
            source.len()
        }
    };
    let start = byte_at(start_line).min(source.len());
    let end = byte_at(end_line_excl).min(source.len());
    if start >= end {
        return "";
    }
    source[start..end].trim()
}

/// Convenience: read + extract, surfacing read errors as a one-off diagnostic.
pub fn extract_file(path: &Path) -> FileExtraction {
    match std::fs::read_to_string(path) {
        Ok(source) => extract(path, &source),
        Err(e) => FileExtraction {
            requirements: Vec::new(),
            errors: vec![miette!("failed to read {}: {e}", path.display())],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn extract_str(src: &str) -> FileExtraction {
        extract(&PathBuf::from("CHECKS.md"), src)
    }

    #[test]
    fn requirement_with_two_checks_keeps_order_and_verbatim_prompts() {
        let src = "# Requirement Images small\nintro prose\n\n## Check JPegs\nstat the jpgs\nflag big ones\n\n## Check SVGs\nstat the svgs\n";
        let out = extract_str(src);
        assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
        assert_eq!(out.requirements.len(), 1);
        let req = &out.requirements[0];
        assert_eq!(req.title, "Images small");
        assert_eq!(req.checks.len(), 2);
        assert_eq!(req.checks[0].title, "JPegs");
        assert_eq!(req.checks[0].prompt, "stat the jpgs\nflag big ones");
        assert_eq!(req.checks[1].title, "SVGs");
        assert_eq!(req.checks[1].prompt, "stat the svgs");
    }

    #[test]
    fn req_alias_parses_same_as_requirement() {
        let a = extract_str("# Requirement Foo\n## Check C\ndo it\n");
        let b = extract_str("# Req Foo\n## Check C\ndo it\n");
        assert_eq!(a.requirements[0].title, "Foo");
        assert_eq!(b.requirements[0].title, "Foo");
    }

    #[test]
    fn anonymous_check_inherits_title_and_uses_prose() {
        let src = "# Requirement No Serif Fonts\nScan the CSS files in this directory. Check each font.\nEnsure none of the named fonts are serif fonts.\n";
        let out = extract_str(src);
        assert!(out.errors.is_empty());
        let req = &out.requirements[0];
        assert_eq!(req.checks.len(), 1);
        assert_eq!(req.checks[0].title, "No Serif Fonts");
        assert!(req.checks[0].prompt.starts_with("Scan the CSS files"));
        assert!(req.checks[0].prompt.ends_with("serif fonts."));
    }

    #[test]
    fn explicit_checks_do_not_promote_prose() {
        let src = "# Requirement R\nthis prose is metadata\n## Check C\nthe real prompt\n";
        let out = extract_str(src);
        let req = &out.requirements[0];
        assert_eq!(req.checks.len(), 1);
        assert_eq!(req.checks[0].prompt, "the real prompt");
    }

    #[test]
    fn orphan_check_is_an_error() {
        let out = extract_str("## Check Bar\ndo something\n");
        assert_eq!(out.requirements.len(), 0);
        assert_eq!(out.errors.len(), 1);
        assert!(format!("{:?}", out.errors[0]).contains("orphan"));
    }

    #[test]
    fn checkless_requirement_is_an_error() {
        let out = extract_str("# Requirement Empty\n\n# Requirement Other\ndo a thing\n");
        // "Empty" has no prose and no checks -> error; "Other" is a valid anon check.
        assert_eq!(out.requirements.len(), 1);
        assert_eq!(out.requirements[0].title, "Other");
        assert_eq!(out.errors.len(), 1);
        assert!(format!("{:?}", out.errors[0]).contains("checkless"));
    }

    #[test]
    fn non_sentinel_headings_are_prose() {
        // `# Overview` is not a requirement; it should be ignored as prose and
        // not yield a requirement.
        let out = extract_str("# Overview\nsome words\n");
        assert_eq!(out.requirements.len(), 0);
        assert!(out.errors.is_empty());
    }

    #[test]
    fn multiple_requirements_in_one_file() {
        let src = "# Requirement No JSON\nno json files\n\n# Requirement Small Images\n## Check J\njpgs\n";
        let out = extract_str(src);
        assert!(out.errors.is_empty());
        assert_eq!(out.requirements.len(), 2);
        assert_eq!(out.requirements[0].title, "No JSON");
        assert_eq!(out.requirements[0].checks[0].title, "No JSON");
        assert_eq!(out.requirements[1].title, "Small Images");
    }
}
