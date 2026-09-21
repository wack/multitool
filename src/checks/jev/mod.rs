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

// No caller outside this module's own tests reaches `client`/`error`/`types`
// yet: MULTI-1823 ("Build the Jev verification question from replayed
// evidence") and MULTI-1825 ("Decide `multi check` with Jev under the `jev`
// feature") are the tickets that wire this client into `multi check`. Until
// one of them adds a real caller, nothing is re-exported at this module's top
// level — add `pub use` here (and drop this comment) once something needs it.
mod client;
mod error;
mod types;
