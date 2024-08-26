#![allow(dead_code)]
#![allow(unused_imports)]

pub use backend::Backend;
pub use resource::{ResourcePrototype, ResourceRecord};

use serde::{Deserialize, Serialize};

use self::history::RunHistory;

#[derive(Serialize, Deserialize, Default)]
pub struct State {
    resources: Vec<ResourceRecord>,
}

impl State {
    /// Return the empty state, the state with no resources.
    pub fn empty() -> Self {
        Self::default()
    }

    /// `diff` will create a new state by applying the results
    /// of a run to the current state.
    pub fn diff(&self, _history: &RunHistory) -> State {
        todo!();
    }
}

mod backend;
/// Contains the list of changes made during a run.
mod history;
/// A journal (which is effectively a write-ahead log) stores a log
/// of each operation that occurs during a run.
mod journal;
/// These types model projects and their metadata.
mod project;
/// Models resource data
mod resource;
/// Represents whether an operation was completed, successful, failed, or pending.
mod status;
