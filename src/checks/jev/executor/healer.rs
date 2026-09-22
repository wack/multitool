//! Self-healing plan updates (MULTI-1826): collects, off the verdict path,
//! the updates `JevExecutor` discovers a check's `.check-plan.toml` entry
//! needs, and applies them once, after every check in the run has settled —
//! see the parent module's "Self-healing" docs.
//!
//! [`Healer::queue_agent_escalation`] and [`Healer::queue_refresh`] are both
//! cheap and purely synchronous: neither does any I/O, and neither spawns
//! anything. This is deliberate (code review on the first version of this
//! module): a check's outcome must be visibly settled — reported to the
//! presenter/reporting actors — **before** any replay or Jev call for
//! healing purposes can possibly start, and `tokio::spawn`ing the expensive
//! work from inside `run_check` is not a strong enough barrier for that on a
//! multi-threaded runtime (the spawned task can start running on another
//! worker thread before the spawning call even returns its own result to
//! its caller). So nothing expensive happens until [`Healer::finish`] is
//! called — which `JevExecutor::finalize` only ever does after
//! `crate::checks::run_pipeline` has fully settled *every* check in the run
//! (see that function's docs) — at which point `finish` builds every queued
//! agent-escalation entry (bounded concurrency, each individually time-
//! boxed — see [`HEAL_TIMEOUT`]) and merges the results with whatever
//! `queue_refresh` already recorded.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::checks::config::JevConfig;
use crate::checks::executor::{AgentOutcome, PlanIdentity};
use crate::checks::jev::client::JevClient;
use crate::checks::jev::plan_file::PlanCheck;
use crate::checks::model::Check;
use crate::checks::plan::{self, EntryContext};

/// How many entry-building tasks (`crate::checks::plan::entry_from_outcome` —
/// replay plus up to two Jev calls) [`Healer::finish`] may run concurrently.
/// A small, fixed bound rather than a configurable one: Jev calls are small
/// and fast, and this only throttles fan-out for a suite with many queued
/// escalations — it never affects a verdict (by the time `finish` even
/// starts, every verdict in the run is already settled and reported).
const MAX_CONCURRENT_HEALS: usize = 4;

/// Bounds how long building a single check's entry (replay plus up to two
/// Jev calls, via `crate::checks::plan::entry_from_outcome`) may take before
/// it's abandoned (code review). Without this, a stuck replay (e.g. a
/// pathological `Grep`/`Glob`) or a wedged request could hold
/// [`Healer::finish`] — and therefore `JevExecutor::finalize`, and therefore
/// the whole process's exit — open indefinitely. Deliberately generous, and
/// deliberately *not* [`crate::checks::jev::client`]'s own tighter
/// per-request timeout: a single `entry_from_outcome` call can make two
/// *sequential* Jev requests (main verdict, then the negative control),
/// each independently eligible for the client's own `429`/`529` retry
/// budget, so this needs headroom for both, not just one. Chosen to mirror
/// `crate::checks::config`'s `DEFAULT_AGENT_TIMEOUT` (not reachable from
/// here — it's private) rather than inventing an unrelated number: this is
/// the same "how long is one unit of background work allowed to take"
/// budget the executor already grants a live reasoning agent.
const HEAL_TIMEOUT: Duration = Duration::from_secs(240);

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

/// One agent escalation queued for healing — everything
/// [`build_entry`] needs, recorded synchronously by
/// [`Healer::queue_agent_escalation`] and only actually acted on later, by
/// [`Healer::finish`].
struct PendingHeal {
    ctx: HealContext,
    outcome: AgentOutcome,
    /// [`AgentOutcome::sandbox_root`], already unwrapped — checked once, at
    /// queue time, so [`build_entry`] never has to.
    sandbox_root: PathBuf,
}

/// Collects and applies one run's self-healing plan updates — see the module
/// docs.
pub(super) struct Healer {
    jev_client: Arc<JevClient>,
    jev_config: JevConfig,
    /// Agent escalations queued this run, not yet built — see
    /// [`Healer::queue_agent_escalation`] and [`Healer::finish`].
    pending: Mutex<Vec<PendingHeal>>,
    /// Refresh updates recorded synchronously by [`Healer::queue_refresh`],
    /// grouped by directory — merged with whatever [`Healer::finish`] builds
    /// from [`Self::pending`] and drained by it.
    updates: Mutex<HashMap<PathBuf, Vec<Update>>>,
    /// [`HEAL_TIMEOUT`] in production; overridable in tests
    /// ([`Self::set_heal_timeout_for_test`]) so the timeout-handling path in
    /// [`Self::finish`] can be exercised without waiting the real, generous
    /// production duration.
    heal_timeout: Duration,
}

impl Healer {
    pub(super) fn new(jev_client: Arc<JevClient>, jev_config: JevConfig) -> Self {
        Self {
            jev_client,
            jev_config,
            pending: Mutex::new(Vec::new()),
            updates: Mutex::new(HashMap::new()),
            heal_timeout: HEAL_TIMEOUT,
        }
    }

    /// Test-only: shrink [`Self::heal_timeout`] so a test can make
    /// [`Self::finish`]'s timeout-handling path fire deterministically and
    /// fast, instead of waiting [`HEAL_TIMEOUT`] for real.
    #[cfg(test)]
    pub(super) fn set_heal_timeout_for_test(&mut self, timeout: Duration) {
        self.heal_timeout = timeout;
    }

    /// Record a `ReadsStale` `Satisfied` refresh immediately: unlike
    /// [`Self::queue_agent_escalation`], no extra I/O is needed here — the
    /// caller already made the one live Jev call this update needs — so
    /// this is always safe to record synchronously (it does no work `finish`
    /// would need to defer).
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

    /// Record an agent escalation's outcome for healing — cheap, purely
    /// synchronous (a couple of clones and a `Vec` push), and safe to call
    /// no matter how many are already queued: no I/O, no Jev call, and no
    /// task is spawned here. See the module docs on why entry-building
    /// itself is deferred to [`Self::finish`].
    ///
    /// A no-op when there's nothing to freeze: the outcome carried no
    /// verdict (an agent that exhausted every attempt without reporting has
    /// no trace worth healing from), or no sandbox root (defensive — every
    /// inner executor this crate ships, `CerseiExecutor` and the test
    /// `FakeExecutor`, sets this unconditionally whenever it ran an agent;
    /// see `AgentOutcome::sandbox_root`'s docs — a caller that can't supply
    /// one can't be relativized against, logged and skipped rather than
    /// guessing a root).
    pub(super) fn queue_agent_escalation(&self, ctx: HealContext, outcome: &AgentOutcome) {
        if !outcome.has_verdict() {
            return;
        }
        let Some(sandbox_root) = outcome.sandbox_root.clone() else {
            tracing::warn!(
                check = %ctx.check.title,
                dir = %ctx.identity.dir.display(),
                "agent outcome carried no sandbox root; skipping self-heal for this check",
            );
            return;
        };

        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(PendingHeal {
                ctx,
                outcome: outcome.clone(),
                sandbox_root,
            });
    }

    /// Build every queued agent escalation's entry (bounded concurrency,
    /// each individually time-boxed — see [`MAX_CONCURRENT_HEALS`]/
    /// [`HEAL_TIMEOUT`]), merge the results with whatever
    /// [`Self::queue_refresh`] already recorded, and drain-and-return the
    /// combined updates, grouped by directory.
    ///
    /// This is where all of this run's healing I/O actually happens — see
    /// the module docs. Only ever called by `JevExecutor::finalize`, itself
    /// only ever called by `crate::checks::run` after
    /// `crate::checks::run_pipeline` has returned successfully (an aborted
    /// run skips `finalize` entirely — see that call site's docs), so every
    /// check's outcome is already fully settled and reported by the time
    /// any of this starts. Safe to call at most once per run; a second call
    /// would just find nothing pending and nothing recorded.
    pub(super) async fn finish(&self) -> HashMap<PathBuf, Vec<Update>> {
        let pending =
            std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner));

        let mut built = Vec::with_capacity(pending.len());
        if !pending.is_empty() {
            let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_HEALS));
            let mut handles = Vec::with_capacity(pending.len());
            for heal in pending {
                let jev_client = self.jev_client.clone();
                let jev_config = self.jev_config.clone();
                let semaphore = semaphore.clone();
                let heal_timeout = self.heal_timeout;
                handles.push(tokio::spawn(async move {
                    // Bounds how many of these run concurrently — see
                    // `MAX_CONCURRENT_HEALS`.
                    let _permit = semaphore
                        .acquire_owned()
                        .await
                        .expect("this semaphore is never closed");
                    build_entry(heal, &jev_client, &jev_config, heal_timeout).await
                }));
            }

            for handle in handles {
                match handle.await {
                    Ok(Some(entry)) => built.push(entry),
                    Ok(None) => {} // `build_entry` already logged why.
                    Err(err) => tracing::warn!(
                        error = %err,
                        "a self-heal entry-building task panicked; its check's previous plan entry is left in place",
                    ),
                }
            }
        }

        let mut updates =
            std::mem::take(&mut *self.updates.lock().unwrap_or_else(PoisonError::into_inner));
        for (dir, update) in built {
            updates.entry(dir).or_default().push(update);
        }
        updates
    }
}

/// Build one queued escalation's healed entry via
/// `crate::checks::plan::entry_from_outcome` (replay plus up to two Jev
/// calls), time-boxed to `heal_timeout` ([`HEAL_TIMEOUT`] in production).
/// `None` on any failure — an `entry_from_outcome` error (a replay error, an
/// escaped path, a Jev `Exhausted`/`Transport`/`Unauthorized`/
/// `MissingApiKey`) or the timeout itself — logged here, the one place
/// either can happen; the caller ([`Healer::finish`]) simply drops it,
/// leaving the check's previous plan entry untouched.
async fn build_entry(
    heal: PendingHeal,
    jev_client: &JevClient,
    jev_config: &JevConfig,
    heal_timeout: Duration,
) -> Option<(PathBuf, Update)> {
    let PendingHeal {
        ctx,
        outcome,
        sandbox_root,
    } = heal;

    let declared_in = ctx.declared_in.to_string_lossy().into_owned();
    let entry_ctx = EntryContext {
        requirement_title: &ctx.identity.requirement_title,
        declared_in: &declared_in,
        root: &ctx.root,
        sandbox_root: &sandbox_root,
        jev_client,
        jev_config,
    };

    match tokio::time::timeout(
        heal_timeout,
        plan::entry_from_outcome(&ctx.check, &outcome, &entry_ctx),
    )
    .await
    {
        Ok(Ok(planned)) => {
            let entry = planned.into_plan_check(ctx.identity.check_ordinal);
            Some((
                ctx.identity.dir.clone(),
                Update {
                    source: ctx.identity.source.clone(),
                    req_ordinal: ctx.identity.req_ordinal,
                    check_ordinal: ctx.identity.check_ordinal,
                    requirement_title: ctx.identity.requirement_title.clone(),
                    entry,
                },
            ))
        }
        Ok(Err(err)) => {
            tracing::warn!(
                check = %ctx.check.title,
                dir = %ctx.identity.dir.display(),
                error = %err,
                "failed to self-heal this check's plan entry; leaving the previous entry in place",
            );
            None
        }
        Err(_elapsed) => {
            tracing::warn!(
                check = %ctx.check.title,
                dir = %ctx.identity.dir.display(),
                timeout = ?heal_timeout,
                "self-healing this check's plan entry exceeded its timeout; leaving the previous entry in place",
            );
            None
        }
    }
}
