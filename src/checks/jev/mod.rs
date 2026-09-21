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

// MULTI-1822 ("Replay frozen tool calls in-host and classify plan freshness")
// wires `plan_file`'s `PlanCall`/checksum helpers into `replay`, and
// MULTI-1823 ("Build the Jev verification question from replayed evidence")
// wires `client`/`error`/`types`/`plan_file::Reading` into the new `verify`
// module. MULTI-1824 (`multi plan`) is the first CROSS-MODULE consumer —
// `crate::checks::plan` reaches `client`/`error`/`plan_file`/`replay`/`verify`
// directly (`pub(crate) mod`, not individual `pub use` re-exports: each
// module's own items are already `pub`, so widening the module path itself is
// the one visibility change a cross-module caller needs). `types` stays
// private: nothing outside this module touches the wire shapes directly,
// only through `client`/`verify`.
pub(crate) mod client;
pub(crate) mod error;
pub(crate) mod plan_file;
pub(crate) mod replay;
mod types;
pub(crate) mod verify;
