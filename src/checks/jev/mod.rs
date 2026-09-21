//! The Jev decision engine's HTTP client (MULTI-1819): a typed client for
//! TypeSafe's System One API (<https://docs.typesafe.ai/concepts/system-one.md>).
//!
//! Compiled only under `--features jev` (off by default; see `[features].jev`
//! in `Cargo.toml`). The `[checks.jev]` **config** schema and its validation
//! live unconditionally in [`crate::checks::config`] instead, so a config file
//! carrying a `[checks.jev]` table parses the same way regardless of this
//! feature — only the part that actually talks to TypeSafe is feature-gated.
//!
//! Nothing in this milestone wires [`JevClient`] into `multi check` yet: this
//! ticket lays the foundation (client, wire types, typed errors, config) that
//! MULTI-1823 (the verification question) and MULTI-1825 (deciding `multi
//! check` with Jev) build on.
#![cfg(feature = "jev")]

// No caller outside this module's own tests reaches `client`/`error`/`types`/
// `plan_file` yet: MULTI-1822 ("Replay frozen tool calls in-host and classify
// plan freshness") and MULTI-1824 (`multi plan`) are the tickets that wire
// `plan_file`'s `PlanStore`/`PlanFile` into a real planner, same as
// MULTI-1823/1825 do for `client`. Until one of them adds a real caller,
// nothing from any of the four is re-exported at this module's top level —
// add `pub use` here (and narrow this comment) once something needs it.
mod client;
mod error;
mod plan_file;
mod types;
