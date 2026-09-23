//! Parse a single `CHECKS.toml` and extract its [`Requirement`]s and their
//! [`Check`]s.
//!
//! Schema (version 1):
//!
//! ```toml
//! version = 1
//!
//! [[requirement]]
//! id = "no-yellow-text"            # kebab-case, unique within the file
//! title = "No Yellow Text"
//! description = "..."              # optional; metadata only, never sent to an agent
//! tags = ["style"]                 # optional
//!
//!   [[requirement.check]]
//!   id = "css-colors"              # kebab-case, unique within its requirement
//!   title = "Confirm No Yellow Text"
//!   kind = "prompt"                # optional; "prompt" is the default and only kind
//!   prompt = '''
//!   Scan the CSS files ...
//!   '''
//! ```
//!
//! Every table rejects unknown keys, so a typo is an error rather than a
//! silently ignored field. Every requirement must declare at least one check.
//!
//! Validation errors are collected rather than thrown so the caller can
//! aggregate them across files. Each is a `miette` diagnostic that names the
//! offending file and, where the TOML parser reports one, labels the exact
//! span in the source.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use miette::{Diagnostic, NamedSource, SourceSpan, miette};
use serde::Deserialize;
use thiserror::Error;
use toml::Spanned;

use crate::checks::model::{Check, CheckKind, Requirement, RootSource};

/// The only `CHECKS.toml` schema version this parser understands.
pub const CURRENT_VERSION: u32 = 1;

/// The result of extracting one file: the requirements it declared plus any
/// validation errors (each a ready-to-render `miette` diagnostic).
pub struct FileExtraction {
    pub requirements: Vec<Requirement>,
    pub errors: Vec<miette::Error>,
}

/// A problem with a `CHECKS.toml`, pointing at the offending source span when
/// one is known.
#[derive(Debug, Error, Diagnostic)]
#[error("invalid {file}: {message}")]
struct ChecksFileError {
    file: String,
    message: String,
    #[source_code]
    src: NamedSource<String>,
    #[label("here")]
    span: Option<SourceSpan>,
    #[label("first declared here")]
    first: Option<SourceSpan>,
    #[help]
    help: Option<String>,
}

/// Collects diagnostics for one file, sharing its name and source text.
struct Reporter<'a> {
    file: String,
    source: &'a str,
    errors: Vec<miette::Error>,
}

impl Reporter<'_> {
    fn report(
        &mut self,
        message: impl Into<String>,
        span: Option<std::ops::Range<usize>>,
        help: Option<&str>,
    ) {
        self.report_with_first(message, span, None, help);
    }

    fn report_with_first(
        &mut self,
        message: impl Into<String>,
        span: Option<std::ops::Range<usize>>,
        first: Option<std::ops::Range<usize>>,
        help: Option<&str>,
    ) {
        self.errors.push(
            ChecksFileError {
                file: self.file.clone(),
                message: message.into(),
                src: NamedSource::new(&self.file, self.source.to_string()),
                span: span.map(Into::into),
                first: first.map(Into::into),
                help: help.map(str::to_string),
            }
            .into(),
        );
    }
}

/// Reads just `version`, so an unsupported version is reported precisely
/// rather than as whatever schema mismatch the full parse would hit first.
#[derive(Deserialize)]
struct VersionProbe {
    version: Spanned<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[allow(dead_code)] // Validated via `VersionProbe` before this parse.
    version: u32,
    #[serde(rename = "requirement", default)]
    requirements: Vec<RawRequirement>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequirement {
    id: Spanned<String>,
    title: Spanned<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(rename = "check", default)]
    checks: Vec<RawCheck>,
}

/// A check's on-disk shape: every kind's fields, flattened. `kind` selects
/// which of them apply; [`convert_check`] rejects any that don't. (Serde's
/// internally-tagged enums can't default a missing tag, and
/// `#[serde(flatten)]` disables `deny_unknown_fields`, so this is validated
/// by hand.)
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheck {
    id: Spanned<String>,
    title: Spanned<String>,
    #[serde(default)]
    kind: Option<Spanned<String>>,
    #[serde(default)]
    prompt: Option<Spanned<String>>,
}

const ID_HELP: &str =
    "ids are lowercase kebab-case: letters, digits, and single hyphens (e.g. `no-yellow-text`)";

/// Parse `source` (the contents of `path`) and extract its requirements/checks.
///
/// `repo_root`/`root_source` (MULTI-1834) are the already-resolved repository
/// root for `path` (see [`super::repo_root::resolve`]) and are stamped onto
/// every requirement extracted from this file. Never fails outright —
/// problems are returned as `errors` so the caller can aggregate across all
/// files.
pub fn extract(
    path: &Path,
    source: &str,
    repo_root: PathBuf,
    root_source: RootSource,
) -> FileExtraction {
    let mut reporter = Reporter {
        file: path.display().to_string(),
        source,
        errors: Vec::new(),
    };

    let raw = match parse_raw(source, &mut reporter) {
        Some(raw) => raw,
        None => {
            return FileExtraction {
                requirements: Vec::new(),
                errors: reporter.errors,
            };
        }
    };

    let mut requirements = Vec::new();
    let mut seen_requirements: HashMap<String, std::ops::Range<usize>> = HashMap::new();
    for raw_req in raw.requirements {
        let id_ok = validate_id(&raw_req.id, "requirement", &mut reporter);
        if id_ok && let Some(first) = seen_requirements.get(raw_req.id.get_ref()) {
            reporter.report_with_first(
                format!("duplicate requirement id `{}`", raw_req.id.get_ref()),
                Some(raw_req.id.span()),
                Some(first.clone()),
                Some("requirement ids must be unique within a file"),
            );
        } else {
            seen_requirements.insert(raw_req.id.get_ref().clone(), raw_req.id.span());
        }
        let title_ok = validate_title(&raw_req.title, "requirement", &mut reporter);

        if raw_req.checks.is_empty() {
            reporter.report(
                format!("requirement `{}` declares no checks", raw_req.id.get_ref()),
                Some(raw_req.id.span()),
                Some("add at least one `[[requirement.check]]` beneath it"),
            );
        }

        let mut checks = Vec::new();
        let mut seen_checks: HashMap<String, std::ops::Range<usize>> = HashMap::new();
        for raw_check in raw_req.checks {
            let check_id_ok = validate_id(&raw_check.id, "check", &mut reporter);
            if check_id_ok && let Some(first) = seen_checks.get(raw_check.id.get_ref()) {
                reporter.report_with_first(
                    format!(
                        "duplicate check id `{}` in requirement `{}`",
                        raw_check.id.get_ref(),
                        raw_req.id.get_ref()
                    ),
                    Some(raw_check.id.span()),
                    Some(first.clone()),
                    Some("check ids must be unique within their requirement"),
                );
            } else {
                seen_checks.insert(raw_check.id.get_ref().clone(), raw_check.id.span());
            }
            if let Some(check) = convert_check(raw_check, &mut reporter) {
                checks.push(check);
            }
        }

        if id_ok && title_ok && !checks.is_empty() {
            requirements.push(Requirement {
                filepath: path.to_path_buf(),
                id: raw_req.id.into_inner(),
                title: raw_req.title.into_inner().trim().to_string(),
                description: raw_req
                    .description
                    .map(|d| d.trim().to_string())
                    .filter(|d| !d.is_empty()),
                tags: raw_req.tags,
                checks,
                root: repo_root.clone(),
                root_source,
            });
        }
    }

    // Any error anywhere in the file withholds all of its requirements: a
    // half-loaded file would silently skip checks.
    if !reporter.errors.is_empty() {
        requirements.clear();
    }
    FileExtraction {
        requirements,
        errors: reporter.errors,
    }
}

/// Check the schema version, then deserialize the whole file.
fn parse_raw(source: &str, reporter: &mut Reporter<'_>) -> Option<RawFile> {
    let probe: VersionProbe = match toml::from_str(source) {
        Ok(probe) => probe,
        Err(err) => {
            reporter.report(
                err.message().to_string(),
                err.span(),
                Some("every CHECKS.toml must start with `version = 1`"),
            );
            return None;
        }
    };
    if *probe.version.get_ref() != CURRENT_VERSION {
        reporter.report(
            format!(
                "unsupported version {} (expected {CURRENT_VERSION})",
                probe.version.get_ref()
            ),
            Some(probe.version.span()),
            None,
        );
        return None;
    }

    match toml::from_str(source) {
        Ok(raw) => Some(raw),
        Err(err) => {
            reporter.report(err.message().to_string(), err.span(), None);
            None
        }
    }
}

/// Convert one raw check into a [`Check`], validating its title and the
/// fields its `kind` requires.
fn convert_check(raw: RawCheck, reporter: &mut Reporter<'_>) -> Option<Check> {
    let title_ok = validate_title(&raw.title, "check", reporter);
    let kind_name = raw.kind.as_ref().map_or("prompt", |k| k.get_ref().as_str());
    let kind = match kind_name {
        "prompt" => {
            let Some(prompt) = raw.prompt else {
                reporter.report(
                    format!("check `{}` is missing `prompt`", raw.id.get_ref()),
                    Some(raw.id.span()),
                    Some("a `prompt` check needs a `prompt = '''...'''` for the agent"),
                );
                return None;
            };
            let text = prompt.get_ref().trim();
            if text.is_empty() {
                reporter.report(
                    format!("check `{}` has an empty `prompt`", raw.id.get_ref()),
                    Some(prompt.span()),
                    None,
                );
                return None;
            }
            CheckKind::Prompt {
                prompt: text.to_string(),
            }
        }
        other => {
            let span = raw.kind.as_ref().map(Spanned::span);
            reporter.report(
                format!("unknown check kind `{other}`"),
                span,
                Some("the only supported kind is `\"prompt\"`"),
            );
            return None;
        }
    };
    title_ok.then(|| Check {
        id: raw.id.into_inner(),
        title: raw.title.into_inner().trim().to_string(),
        kind,
    })
}

/// Report (and return `false` for) an id that isn't lowercase kebab-case.
fn validate_id(id: &Spanned<String>, what: &str, reporter: &mut Reporter<'_>) -> bool {
    if is_kebab_case(id.get_ref()) {
        return true;
    }
    reporter.report(
        format!("invalid {what} id `{}`", id.get_ref()),
        Some(id.span()),
        Some(ID_HELP),
    );
    false
}

/// Report (and return `false` for) a blank title.
fn validate_title(title: &Spanned<String>, what: &str, reporter: &mut Reporter<'_>) -> bool {
    if !title.get_ref().trim().is_empty() {
        return true;
    }
    reporter.report(format!("{what} title is empty"), Some(title.span()), None);
    false
}

/// `^[a-z0-9]+(-[a-z0-9]+)*$`.
fn is_kebab_case(id: &str) -> bool {
    !id.is_empty()
        && id.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Convenience: read + extract, surfacing read errors as a one-off diagnostic.
pub fn extract_file(path: &Path, repo_root: PathBuf, root_source: RootSource) -> FileExtraction {
    match std::fs::read_to_string(path) {
        Ok(source) => extract(path, &source, repo_root, root_source),
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
        extract(
            &PathBuf::from("CHECKS.toml"),
            src,
            PathBuf::from("."),
            RootSource::ScanDirectory,
        )
    }

    fn rendered_errors(out: &FileExtraction) -> String {
        out.errors
            .iter()
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn requirement_with_two_checks_keeps_order_and_fields() {
        let src = r#"
version = 1

[[requirement]]
id = "images-small"
title = "Images small"
description = """
  intro prose
"""
tags = ["assets", "perf"]

[[requirement.check]]
id = "jpegs"
title = "JPegs"
kind = "prompt"
prompt = '''
stat the jpgs
flag big ones
'''

[[requirement.check]]
id = "svgs"
title = "SVGs"
prompt = "stat the svgs"
"#;
        let out = extract_str(src);
        assert!(out.errors.is_empty(), "errors: {}", rendered_errors(&out));
        assert_eq!(out.requirements.len(), 1);
        let req = &out.requirements[0];
        assert_eq!(req.id, "images-small");
        assert_eq!(req.title, "Images small");
        assert_eq!(req.description.as_deref(), Some("intro prose"));
        assert_eq!(req.tags, vec!["assets", "perf"]);
        assert_eq!(req.checks.len(), 2);
        assert_eq!(req.checks[0].id, "jpegs");
        assert_eq!(req.checks[0].title, "JPegs");
        assert_eq!(req.checks[0].prompt(), "stat the jpgs\nflag big ones");
        assert_eq!(req.checks[1].id, "svgs");
        // `kind` defaults to "prompt".
        assert_eq!(req.checks[1].prompt(), "stat the svgs");
    }

    #[test]
    fn multiple_requirements_keep_declaration_order() {
        let src = r#"
version = 1

[[requirement]]
id = "b"
title = "B"
  [[requirement.check]]
  id = "c"
  title = "C"
  prompt = "do b"

[[requirement]]
id = "a"
title = "A"
  [[requirement.check]]
  id = "c"
  title = "C"
  prompt = "do a"
"#;
        let out = extract_str(src);
        assert!(out.errors.is_empty(), "errors: {}", rendered_errors(&out));
        let ids: Vec<_> = out.requirements.iter().map(|r| r.id.as_str()).collect();
        // Check ids only need to be unique within their requirement.
        assert_eq!(ids, vec!["b", "a"]);
    }

    #[test]
    fn empty_file_with_only_a_version_is_valid() {
        let out = extract_str("version = 1\n");
        assert!(out.errors.is_empty());
        assert!(out.requirements.is_empty());
    }

    #[test]
    fn missing_version_is_an_error_naming_the_file() {
        let out = extract_str("[[requirement]]\nid = \"a\"\n");
        assert_eq!(out.errors.len(), 1);
        let msg = rendered_errors(&out);
        assert!(msg.contains("CHECKS.toml"), "{msg}");
        assert!(msg.contains("version"), "{msg}");
    }

    #[test]
    fn unsupported_version_is_an_error() {
        let out = extract_str("version = 2\n");
        assert_eq!(out.errors.len(), 1);
        assert!(rendered_errors(&out).contains("unsupported version 2"));
    }

    #[test]
    fn checkless_requirement_is_an_error_naming_the_file() {
        let src = r#"
version = 1

[[requirement]]
id = "empty"
title = "Empty"
"#;
        let out = extract_str(src);
        assert!(out.requirements.is_empty());
        assert_eq!(out.errors.len(), 1);
        let msg = rendered_errors(&out);
        assert!(msg.contains("CHECKS.toml"), "{msg}");
        assert!(msg.contains("declares no checks"), "{msg}");
    }

    #[test]
    fn unknown_field_is_an_error() {
        let src = r#"
version = 1

[[requirement]]
id = "r"
title = "R"
  [[requirement.check]]
  id = "c"
  title = "C"
  promt = "typo"
"#;
        let out = extract_str(src);
        assert!(out.requirements.is_empty());
        assert!(rendered_errors(&out).contains("promt"));
    }

    #[test]
    fn missing_prompt_is_an_error() {
        let src = r#"
version = 1

[[requirement]]
id = "r"
title = "R"
  [[requirement.check]]
  id = "c"
  title = "C"
"#;
        let out = extract_str(src);
        assert!(out.requirements.is_empty());
        assert!(rendered_errors(&out).contains("missing `prompt`"));
    }

    #[test]
    fn blank_prompt_is_an_error() {
        let src = r#"
version = 1

[[requirement]]
id = "r"
title = "R"
  [[requirement.check]]
  id = "c"
  title = "C"
  prompt = "   "
"#;
        let out = extract_str(src);
        assert!(rendered_errors(&out).contains("empty `prompt`"));
    }

    #[test]
    fn unknown_kind_is_an_error() {
        let src = r#"
version = 1

[[requirement]]
id = "r"
title = "R"
  [[requirement.check]]
  id = "c"
  title = "C"
  kind = "shell"
  prompt = "x"
"#;
        let out = extract_str(src);
        assert!(rendered_errors(&out).contains("unknown check kind `shell`"));
    }

    #[test]
    fn non_kebab_case_ids_are_errors() {
        for bad in [
            "Upper",
            "under_score",
            "-leading",
            "trailing-",
            "double--dash",
            "",
            "sp ace",
        ] {
            let src = format!(
                "version = 1\n[[requirement]]\nid = {bad:?}\ntitle = \"R\"\n[[requirement.check]]\nid = \"c\"\ntitle = \"C\"\nprompt = \"x\"\n"
            );
            let out = extract_str(&src);
            assert!(
                rendered_errors(&out).contains("invalid requirement id"),
                "{bad:?} should be rejected"
            );
        }
        assert!(is_kebab_case("a1-b2-c3"));
    }

    #[test]
    fn duplicate_requirement_ids_are_errors() {
        let src = r#"
version = 1

[[requirement]]
id = "dup"
title = "One"
  [[requirement.check]]
  id = "c"
  title = "C"
  prompt = "x"

[[requirement]]
id = "dup"
title = "Two"
  [[requirement.check]]
  id = "c"
  title = "C"
  prompt = "y"
"#;
        let out = extract_str(src);
        assert!(out.requirements.is_empty());
        assert_eq!(out.errors.len(), 1);
        assert!(rendered_errors(&out).contains("duplicate requirement id `dup`"));
    }

    #[test]
    fn duplicate_check_ids_within_a_requirement_are_errors() {
        let src = r#"
version = 1

[[requirement]]
id = "r"
title = "R"
  [[requirement.check]]
  id = "same"
  title = "A"
  prompt = "x"
  [[requirement.check]]
  id = "same"
  title = "B"
  prompt = "y"
"#;
        let out = extract_str(src);
        assert!(rendered_errors(&out).contains("duplicate check id `same`"));
    }

    #[test]
    fn duplicate_titles_are_allowed() {
        let src = r#"
version = 1

[[requirement]]
id = "one"
title = "Dup"
  [[requirement.check]]
  id = "a"
  title = "Same"
  prompt = "x"
  [[requirement.check]]
  id = "b"
  title = "Same"
  prompt = "y"

[[requirement]]
id = "two"
title = "Dup"
  [[requirement.check]]
  id = "a"
  title = "Same"
  prompt = "z"
"#;
        let out = extract_str(src);
        assert!(out.errors.is_empty(), "errors: {}", rendered_errors(&out));
        assert_eq!(out.requirements.len(), 2);
        assert_eq!(out.requirements[0].checks.len(), 2);
    }

    #[test]
    fn one_bad_requirement_withholds_the_whole_file() {
        let src = r#"
version = 1

[[requirement]]
id = "good"
title = "Good"
  [[requirement.check]]
  id = "c"
  title = "C"
  prompt = "x"

[[requirement]]
id = "bad"
title = "Bad"
"#;
        let out = extract_str(src);
        assert!(out.requirements.is_empty());
        assert_eq!(out.errors.len(), 1);
    }

    /// The repository's own self-validation suite must stay loadable.
    #[test]
    fn repository_checks_toml_is_valid() {
        let out = extract_str(include_str!("../../../CHECKS.toml"));
        assert!(out.errors.is_empty(), "errors: {}", rendered_errors(&out));
        assert!(!out.requirements.is_empty());
    }

    #[test]
    fn syntax_error_is_reported_with_a_span() {
        let out = extract_str("version = 1\n[[requirement]\n");
        assert_eq!(out.errors.len(), 1);
        let diag = out.errors[0]
            .labels()
            .map(|labels| labels.count())
            .unwrap_or(0);
        assert!(diag > 0, "expected a labeled span");
    }
}
