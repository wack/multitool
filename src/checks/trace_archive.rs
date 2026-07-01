//! In-memory assembly of the opt-in session-trace archive
//! (`multi check --trace-archive <PATH>`).
//!
//! Each check *execution* — including every retry — contributes one NDJSON trace
//! produced by the executor (see [`crate::checks::executor::trace`]) and carried
//! back in [`AgentOutcome::trace_jsonl`]. The execution actor pushes each one
//! into a shared [`TraceCollector`], tagged with its requirement, check title,
//! and 1-based attempt number. When the run finishes, [`TraceCollector::into_tar_gz`]
//! lays the traces out as
//!
//! ```text
//! {NN}-{requirement-slug}/{check-slug}.attempt-{k}.jsonl
//! ```
//!
//! — one directory per requirement, one file per (check, attempt) — and streams
//! them through a `tar` builder wrapped in a `flate2` gzip encoder, entirely in
//! memory: pure Rust, no temp directory, no `tar`/`gzip` subprocess.
//!
//! [`AgentOutcome::trace_jsonl`]: crate::checks::executor::AgentOutcome::trace_jsonl

use std::collections::HashSet;
use std::io::Write;
use std::sync::Mutex;

use flate2::Compression;
use flate2::write::GzEncoder;
use miette::{IntoDiagnostic, Result};
use tar::{Builder, Header};

use crate::checks::model::CheckId;

/// One captured execution trace plus the metadata that places it in the archive.
pub(crate) struct TraceEntry {
    /// The requirement's declaration-order index (its archive directory).
    pub req_index: usize,
    /// The requirement title (slugged into the directory name).
    pub req_title: String,
    /// The check's run-unique id, used only to disambiguate a same-title clash.
    pub check_id: CheckId,
    /// The check title (slugged into the file name).
    pub check_title: String,
    /// 1-based attempt number; retries increment it.
    pub attempt: usize,
    /// The self-contained NDJSON trace document for this one execution.
    pub bytes: Vec<u8>,
}

/// Thread-safe sink the execution tasks push into as each attempt completes.
/// Shared behind an `Arc` across the concurrent per-check tasks.
#[derive(Default)]
pub(crate) struct TraceCollector {
    entries: Mutex<Vec<TraceEntry>>,
}

impl TraceCollector {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record one execution's trace. Never blocks meaningfully — a brief lock on
    /// a `Vec` push.
    pub(crate) fn push(&self, entry: TraceEntry) {
        self.lock().push(entry);
    }

    /// How many execution traces have been collected.
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no execution traces have been collected.
    pub(crate) fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Consume the collected traces into a gzip-compressed tar archive, built
    /// entirely in memory.
    pub(crate) fn into_tar_gz(&self) -> Result<Vec<u8>> {
        let mut entries = std::mem::take(&mut *self.lock());
        // Deterministic layout: group by requirement, then check, then attempt.
        entries.sort_by(|a, b| {
            a.req_index
                .cmp(&b.req_index)
                .then(a.check_id.cmp(&b.check_id))
                .then(a.attempt.cmp(&b.attempt))
        });

        let mut used_paths: HashSet<String> = HashSet::new();
        let mut tar = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));

        for entry in &entries {
            let dir = format!("{:02}-{}", entry.req_index + 1, slug(&entry.req_title));
            let base = slug(&entry.check_title);
            let mut path = format!("{dir}/{base}.attempt-{}.jsonl", entry.attempt);
            // Two distinct checks under one requirement can share a title (e.g.
            // both inherited it from the requirement), colliding on the same
            // (title, attempt) path. Disambiguate the loser with its check id.
            if !used_paths.insert(path.clone()) {
                path = format!(
                    "{dir}/{base}.check-{}.attempt-{}.jsonl",
                    entry.check_id, entry.attempt
                );
                used_paths.insert(path.clone());
            }

            let mut header = Header::new_gnu();
            header.set_size(entry.bytes.len() as u64);
            header.set_mode(0o644);
            // Fixed mtime keeps the archive byte-reproducible for a given input.
            header.set_mtime(0);
            header.set_cksum();
            tar.append_data(&mut header, &path, entry.bytes.as_slice())
                .into_diagnostic()?;
        }

        // `into_inner` finishes the tar (writing its trailer), then `finish`
        // flushes the gzip trailer and hands back the compressed bytes.
        let gz = tar.into_inner().into_diagnostic()?;
        gz.finish().into_diagnostic()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<TraceEntry>> {
        // A poisoned lock only means a task panicked mid-push; recover the guard
        // rather than propagating the panic into the archive step.
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Slugify a title into one filesystem-safe path segment: lowercase alphanumerics
/// kept, every other run collapsed to a single `-`, ends trimmed. Empty input (or
/// all-punctuation) becomes `untitled`.
fn slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Build the archive from `collector` and write it to `path`, creating parent
/// directories as needed. A single self-contained step so the orchestrator can
/// treat trace archiving as best-effort: failures are surfaced as `Err` for the
/// caller to log without failing the check run.
pub(crate) fn write_archive(collector: &TraceCollector, path: &std::path::Path) -> Result<usize> {
    let count = collector.len();
    let bytes = collector.into_tar_gz()?;
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).into_diagnostic()?;
    }
    let mut file = std::fs::File::create(path).into_diagnostic()?;
    file.write_all(&bytes).into_diagnostic()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        req_index: usize,
        req_title: &str,
        check_id: CheckId,
        check_title: &str,
        attempt: usize,
    ) -> TraceEntry {
        TraceEntry {
            req_index,
            req_title: req_title.to_string(),
            check_id,
            check_title: check_title.to_string(),
            attempt,
            bytes: format!("{{\"event\":\"header\",\"check_id\":{check_id}}}\n").into_bytes(),
        }
    }

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(slug("No YELLOW text!"), "no-yellow-text");
        assert_eq!(slug("  spaces  "), "spaces");
        assert_eq!(slug("a/b\\c"), "a-b-c");
        assert_eq!(slug("***"), "untitled");
        assert_eq!(slug(""), "untitled");
    }

    #[test]
    fn archive_round_trips_expected_paths() {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let collector = TraceCollector::new();
        // Requirement 0 has one check run twice (attempt 1 then 2).
        collector.push(entry(0, "First req", 0, "Check A", 1));
        collector.push(entry(0, "First req", 0, "Check A", 2));
        // Requirement 1 has two checks that share a title (collision path).
        collector.push(entry(1, "Second req", 1, "Same", 1));
        collector.push(entry(1, "Second req", 2, "Same", 1));

        assert_eq!(collector.len(), 4);
        let gz = collector.into_tar_gz().unwrap();
        // Draining left the collector empty.
        assert_eq!(collector.len(), 0);

        let mut tar_bytes = Vec::new();
        GzDecoder::new(&gz[..]).read_to_end(&mut tar_bytes).unwrap();
        let mut archive = tar::Archive::new(&tar_bytes[..]);
        let mut paths: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        paths.sort();

        assert_eq!(
            paths,
            vec![
                "01-first-req/check-a.attempt-1.jsonl".to_string(),
                "01-first-req/check-a.attempt-2.jsonl".to_string(),
                // The second check with the same title is disambiguated by id.
                "02-second-req/same.attempt-1.jsonl".to_string(),
                "02-second-req/same.check-2.attempt-1.jsonl".to_string(),
            ]
        );
    }
}
