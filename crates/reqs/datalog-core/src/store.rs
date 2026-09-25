use std::future::Future;

use crate::ast::{Decl, Fact, Program, Rule};
use crate::eval::Model;

/// An edit to the stored program. Stores apply a batch of changes atomically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// Insert or replace a declaration by name.
    PutDecl(Decl),
    RemoveDecl(String),
    /// Insert or replace a rule by name (replacing keeps its position).
    PutRule(Rule),
    RemoveRule(String),
    RenameRule {
        from: String,
        to: String,
    },
    AddFact(Fact),
    RemoveFact(Fact),
}

/// A stored model and whether the program changed after it was computed.
#[derive(Clone, Debug)]
pub struct ModelSnapshot {
    pub model: Model,
    pub stale: bool,
}

/// Persistence for programs and computed models.
///
/// Implementations keep a program generation counter that every non-empty
/// `apply` bumps; `save_model` records the generation it was computed from,
/// which is how `load_model` reports staleness.
pub trait Store {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Load declarations, rules (in insertion order), and asserted facts.
    fn load_program(&self) -> impl Future<Output = Result<Program, Self::Error>> + Send;

    /// Apply changes atomically. Callers validate the resulting program first.
    fn apply(&self, changes: &[Change]) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Replace the stored model.
    fn save_model(&self, model: &Model) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// The last saved model, if any.
    fn load_model(&self)
    -> impl Future<Output = Result<Option<ModelSnapshot>, Self::Error>> + Send;
}
