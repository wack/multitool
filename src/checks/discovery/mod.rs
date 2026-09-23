//! The discovery phase: find `CHECKS.toml` files, parse and validate each,
//! and assemble the final `Vec<Requirement>` consumed by execution.
//!
//! File loading + parsing run in parallel (one blocking task per file); this
//! module joins the results in deterministic (sorted-file) order and aggregates
//! all validation errors into a single diagnostic rather than failing fast, for
//! authoring ergonomics.

mod parse;
mod repo_root;
mod walk;

use std::path::{Path, PathBuf};

use kameo::Actor;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use miette::{IntoDiagnostic, Result};
use multi_core::ManyError;

use crate::checks::execution::ExecutionActor;
use crate::checks::messages::{BeginDiscovery, DiscoveryFailed};
use crate::checks::model::Requirement;
use crate::checks::presenter::PresenterActor;
use crate::checks::reporting::ReportingActor;

/// Run the full discovery phase rooted at `root`.
///
/// Returns the validated requirement set (every requirement guaranteed to have
/// ≥1 check), or an aggregated diagnostic naming every offending file/line. An
/// empty tree yields an empty set (the pipeline then succeeds with exit 0).
///
/// `root` is frequently **relative** — `multi check` defaults it to `.` and
/// accepts relative arguments like `services/keystore` — but repository-root
/// resolution (MULTI-1834) needs an absolute anchor: a relative path's
/// ancestors never climb above it, so a relative `root` could never discover
/// a manifest above the scan directory (exactly the invocation-dependence
/// this ticket exists to remove), and its fallback root would itself be
/// relative, breaking the sandbox/`declared_in` machinery downstream that
/// assumes `Requirement::root` is always absolute. `scan_root` is therefore
/// resolved to absolute once, here, for root resolution/sandboxing only. The
/// `CHECKS.toml` paths [`walk::find_checks_files`] discovers (and thus
/// `Requirement::filepath`) are left exactly as `root` produced them, so any
/// diagnostic naming a file (a malformed-file error, for instance) keeps
/// displaying the same path the user typed — unaffected by this ticket.
pub async fn discover(root: &Path) -> Result<Vec<Requirement>> {
    let files = walk::find_checks_files(root)?;
    let scan_root = std::path::absolute(root).into_diagnostic()?;

    // Parse + extract each file in parallel on blocking tasks (file IO + CPU).
    // Repository-root resolution (MULTI-1834) rides along on the same
    // blocking task: it's a handful of `fs::metadata` calls walking up from
    // the file's own directory, cheap but still blocking I/O.
    let handles: Vec<_> = files
        .into_iter()
        .map(|path| {
            let scan_root = scan_root.clone();
            tokio::task::spawn_blocking(move || {
                let (req_root, root_source) = repo_root::resolve(&path, &scan_root);
                parse::extract_file(&path, req_root, root_source)
            })
        })
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

/// The discovery actor: on a [`BeginDiscovery`] kick it runs the whole-suite
/// parse + validation gate ([`discover`]) and then **streams** the validated
/// checks downstream — assigning each a run-unique [`CheckId`] and `tell`ing one
/// [`CheckDiscovered`] per check, followed by a [`DiscoveryComplete`] sentinel.
///
/// If the suite is invalid it streams **nothing** and instead tells reporting to
/// abort the whole run (strict whole-run abort, decision #3), so no agents are
/// ever spawned for a suite that will not run.
///
/// [`CheckId`]: crate::checks::model::CheckId
/// [`CheckDiscovered`]: crate::checks::messages::CheckDiscovered
/// [`DiscoveryComplete`]: crate::checks::messages::DiscoveryComplete
pub(crate) struct DiscoveryActor {
    root: PathBuf,
    execution: ActorRef<ExecutionActor>,
    reporting: ActorRef<ReportingActor>,
    presenter: ActorRef<PresenterActor>,
}

impl Actor for DiscoveryActor {
    type Args = Self;
    type Error = std::convert::Infallible;

    async fn on_start(
        args: Self::Args,
        _actor_ref: ActorRef<Self>,
    ) -> std::result::Result<Self, Self::Error> {
        Ok(args)
    }
}

impl DiscoveryActor {
    pub(crate) fn new(
        root: PathBuf,
        execution: ActorRef<ExecutionActor>,
        reporting: ActorRef<ReportingActor>,
        presenter: ActorRef<PresenterActor>,
    ) -> Self {
        Self {
            root,
            execution,
            reporting,
            presenter,
        }
    }
}

impl Message<BeginDiscovery> for DiscoveryActor {
    type Reply = ();

    async fn handle(&mut self, _msg: BeginDiscovery, _ctx: &mut Context<Self, ()>) -> Self::Reply {
        match discover(&self.root).await {
            Ok(requirements) => {
                if let Err(err) = crate::checks::stream_requirements(
                    &self.execution,
                    &self.presenter,
                    &requirements,
                )
                .await
                {
                    tracing::error!(?err, "failed to stream discovered checks to execution");
                }
            }
            Err(report) => {
                // Strict whole-run abort: no `CheckDiscovered` is emitted, so no
                // agents run; reporting turns this into the run's `Err`.
                if let Err(err) = self.reporting.tell(DiscoveryFailed { report }).await {
                    tracing::error!(?err, "reporting actor unavailable for discovery failure");
                }
            }
        }
    }
}

/// Render a minimal valid `CHECKS.toml` for tests: each requirement is
/// `(title, checks)` and each check is `(title, prompt)`. Ids are the
/// slugified titles (see [`slug`]), so titles must be unique within their
/// scope — tests that need duplicate titles write the TOML by hand.
#[cfg(test)]
pub(crate) fn checks_toml(requirements: &[(&str, &[(&str, &str)])]) -> String {
    let mut out = String::from("version = 1\n");
    for (req_title, checks) in requirements {
        out.push_str(&format!(
            "\n[[requirement]]\nid = {:?}\ntitle = {:?}\n",
            slug(req_title),
            req_title
        ));
        for (check_title, prompt) in *checks {
            out.push_str(&format!(
                "\n[[requirement.check]]\nid = {:?}\ntitle = {:?}\nprompt = {:?}\n",
                slug(check_title),
                check_title,
                prompt
            ));
        }
    }
    out
}

/// Render `err` as miette does, with its line wrapping undone, so tests can
/// match a message regardless of where the renderer broke it.
#[cfg(test)]
pub(crate) fn unwrapped(err: &miette::Report) -> String {
    format!("{err:?}")
        .split_whitespace()
        .filter(|word| *word != "│")
        .collect::<Vec<_>>()
        .join(" ")
}

/// Kebab-case `title` into a valid id (test helper for [`checks_toml`]).
#[cfg(test)]
pub(crate) fn slug(title: &str) -> String {
    title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn valid_multi_file_tree_returns_all_requirements() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("CHECKS.toml"),
            checks_toml(&[("A", &[("A", "do a")])]),
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(
            dir.path().join("sub/CHECKS.toml"),
            checks_toml(&[("B", &[("B1", "b1"), ("B2", "b2")])]),
        )
        .unwrap();

        let reqs = discover(dir.path()).await.unwrap();
        assert_eq!(reqs.len(), 2);
        // Each requirement has at least one check.
        assert!(reqs.iter().all(|r| !r.checks.is_empty()));
        // Sorted-file order: root CHECKS.toml ("A") before sub/CHECKS.toml ("B").
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
    async fn checkless_requirement_aborts_discovery() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("CHECKS.toml"),
            checks_toml(&[("Lonely", &[])]),
        )
        .unwrap();
        let err = discover(dir.path()).await.unwrap_err();
        assert!(unwrapped(&err).contains("declares no checks"));
    }
}
