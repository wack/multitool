//! Self-healing plan updates (MULTI-1826): collects, off the verdict path,
//! the updates `JevExecutor` discovers a check's `.check-plan.toml` entry
//! needs, and applies them once, after every check in the run has settled —
//! see the parent module's "Self-healing" docs.
//!
//! [`Healer::queue_agent_escalation`] is the expensive path: it spawns a
//! bounded background task that replays the agent's captured calls and
//! calibrates against Jev (`crate::checks::plan::entry_from_outcome` —
//! exactly what `multi plan` itself would have written) and only then
//! records the result. [`Healer::queue_refresh`] is the cheap path — a live
//! `ReadsStale` `Satisfied` settlement already did all the I/O this update
//! needs — so it records immediately, synchronously, no task spawned.
//! [`Healer::finish`] awaits every outstanding task and drains every
//! collected update, grouped by directory, for the caller
//! (`JevExecutor::finalize`) to merge onto each directory's plan and write.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::checks::config::JevConfig;
use crate::checks::executor::{AgentOutcome, PlanIdentity};
use crate::checks::jev::client::JevClient;
use crate::checks::jev::plan_file::PlanCheck;
use crate::checks::model::Check;
use crate::checks::plan::{self, EntryContext};

/// How many entry-building tasks (`crate::checks::plan::entry_from_outcome` —
/// replay plus up to two Jev calls) may run concurrently, independent of the
/// check-level `checks.concurrency` limit: a check's own dispatch "slot"
/// frees the instant its agent reports, well before its healing task
/// finishes (see `super::JevExecutor::run_agent`), so without a bound of its
/// own a suite with many simultaneous agent escalations could otherwise open
/// one outstanding Jev request per escalation, all at once. A small, fixed
/// bound rather than a configurable one: Jev calls are small and fast, and
/// this only throttles fan-out — it never blocks a verdict.
const MAX_CONCURRENT_HEALS: usize = 4;

/// One check's fully-resolved healed entry, positioned within its
/// `.check-plan.toml` — [`super::JevExecutor::finalize`] groups these by
/// directory (via [`Healer::finish`]) before applying them.
pub(super) struct Update {
    pub(super) source: String,
    pub(super) req_ordinal: u32,
    pub(super) check_ordinal: u32,
    pub(super) requirement_title: String,
    pub(super) entry: PlanCheck,
}

/// Everything [`Healer::queue_agent_escalation`] needs to eventually build a
/// healed entry, captured from an
/// [`AgentRunRequest`](crate::checks::executor::AgentRunRequest) before it's
/// moved into `inner.run_check` — see `super::JevExecutor::run_agent`.
pub(super) struct HealContext {
    pub(super) identity: PlanIdentity,
    pub(super) check: Check,
    pub(super) declared_in: PathBuf,
    pub(super) root: PathBuf,
}

/// Collects and applies one run's self-healing plan updates — see the module
/// docs.
pub(super) struct Healer {
    jev_client: Arc<JevClient>,
    jev_config: JevConfig,
    /// Bounds [`Self::queue_agent_escalation`]'s concurrent entry-building
    /// work — see [`MAX_CONCURRENT_HEALS`].
    semaphore: Arc<Semaphore>,
    /// Every entry-building task spawned this run, awaited (and drained) by
    /// [`Self::finish`].
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Every update collected so far, grouped by directory — populated by a
    /// spawned task on success ([`Self::queue_agent_escalation`]) or
    /// synchronously ([`Self::queue_refresh`]); drained by [`Self::finish`].
    updates: Arc<Mutex<HashMap<PathBuf, Vec<Update>>>>,
}

impl Healer {
    pub(super) fn new(jev_client: Arc<JevClient>, jev_config: JevConfig) -> Self {
        Self {
            jev_client,
            jev_config,
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT_HEALS)),
            tasks: Mutex::new(Vec::new()),
            updates: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Record a `ReadsStale` `Satisfied` refresh immediately: unlike
    /// [`Self::queue_agent_escalation`], no extra I/O is needed here — the
    /// caller already made the one live Jev call this update needs — so
    /// this never spawns a task.
    pub(super) fn queue_refresh(&self, identity: &PlanIdentity, entry: PlanCheck) {
        let mut updates = self.updates.lock().unwrap_or_else(PoisonError::into_inner);
        updates
            .entry(identity.dir.clone())
            .or_default()
            .push(Update {
                source: identity.source.clone(),
                req_ordinal: identity.req_ordinal,
                check_ordinal: identity.check_ordinal,
                requirement_title: identity.requirement_title.clone(),
                entry,
            });
    }

    /// Queue an agent escalation's outcome for healing: spawns a bounded
    /// background task that builds the replacement entry via
    /// `crate::checks::plan::entry_from_outcome` (replay plus up to two Jev
    /// calls) and records it on success. A failure — the outcome carried no
    /// verdict or no sandbox root, or `entry_from_outcome` itself errored (a
    /// replay error, an escaped path, a Jev `Exhausted`/`Transport`/
    /// `Unauthorized`/`MissingApiKey`) — is logged and the check's previous
    /// entry is simply never touched; never propagated, since the verdict
    /// this run reported is already final by the time this task even starts.
    pub(super) fn queue_agent_escalation(&self, ctx: HealContext, outcome: &AgentOutcome) {
        if !outcome.has_verdict() {
            // Nothing to freeze — `entry_from_outcome` requires a verdict:
            // an agent that exhausted every attempt without reporting has no
            // trace worth healing from.
            return;
        }
        let Some(sandbox_root) = outcome.sandbox_root.clone() else {
            // Defensive: every inner executor this crate ships
            // (`CerseiExecutor`, the test `FakeExecutor`) sets this
            // unconditionally whenever it ran an agent — see
            // `AgentOutcome::sandbox_root`'s docs. A caller that can't
            // supply it can't be relativized against, so skip rather than
            // guess a root.
            tracing::warn!(
                check = %ctx.check.title,
                dir = %ctx.identity.dir.display(),
                "agent outcome carried no sandbox root; skipping self-heal for this check",
            );
            return;
        };

        let outcome = outcome.clone();
        let jev_client = self.jev_client.clone();
        let jev_config = self.jev_config.clone();
        let semaphore = self.semaphore.clone();
        let updates = self.updates.clone();

        let handle = tokio::spawn(async move {
            // Bounds how many of these run concurrently — see
            // `MAX_CONCURRENT_HEALS`. Acquired here, inside the task, not
            // before spawning: spawning itself is never bounded, only the
            // actual replay/Jev work is.
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("this semaphore is never closed");

            let declared_in = ctx.declared_in.to_string_lossy().into_owned();
            let entry_ctx = EntryContext {
                requirement_title: &ctx.identity.requirement_title,
                declared_in: &declared_in,
                root: &ctx.root,
                sandbox_root: &sandbox_root,
                jev_client: &jev_client,
                jev_config: &jev_config,
            };

            match plan::entry_from_outcome(&ctx.check, &outcome, &entry_ctx).await {
                Ok(planned) => {
                    let entry = planned.into_plan_check(ctx.identity.check_ordinal);
                    let mut updates = updates.lock().unwrap_or_else(PoisonError::into_inner);
                    updates
                        .entry(ctx.identity.dir.clone())
                        .or_default()
                        .push(Update {
                            source: ctx.identity.source.clone(),
                            req_ordinal: ctx.identity.req_ordinal,
                            check_ordinal: ctx.identity.check_ordinal,
                            requirement_title: ctx.identity.requirement_title.clone(),
                            entry,
                        });
                }
                Err(err) => {
                    tracing::warn!(
                        check = %ctx.check.title,
                        dir = %ctx.identity.dir.display(),
                        error = %err,
                        "failed to self-heal this check's plan entry; leaving the previous entry in place",
                    );
                }
            }
        });

        self.tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(handle);
    }

    /// Await every task this run spawned, then drain and return every
    /// collected update, grouped by directory. A task that panicked is
    /// logged and otherwise ignored — it recorded nothing before panicking,
    /// so there's nothing to drop from the collector. Safe to call at most
    /// once per run (`JevExecutor::finalize`'s only caller); a second call
    /// would just find no outstanding tasks and an empty collector.
    pub(super) async fn finish(&self) -> HashMap<PathBuf, Vec<Update>> {
        let handles =
            std::mem::take(&mut *self.tasks.lock().unwrap_or_else(PoisonError::into_inner));
        for handle in handles {
            if let Err(err) = handle.await {
                tracing::warn!(
                    error = %err,
                    "a self-heal entry-building task panicked; its check's previous plan entry is left in place",
                );
            }
        }
        std::mem::take(&mut *self.updates.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
