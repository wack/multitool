//! Requirements as AND-OR graphs, compiled into core Datalog.
//!
//! Requirement predicates are declared with `@requirement`. A rule tagged
//! `@requirement` decomposes its head into the requirement atoms in its body
//! (an AND). Several such rules for the same head are alternatives (an OR).
//! All other body literals are *guards*: they choose which instantiations
//! exist, and must bind every variable.
//!
//! See [`compile`] for the translation and [`report`] for reading results.

pub mod compile;
pub mod report;

pub use compile::{
    CompileError, Compiled, PRELUDE, REQUIREMENT, RESERVED, compile, requirement_decl,
};
pub use report::{AltStatus, Alternative, AtomStatus, Report, Stance};
