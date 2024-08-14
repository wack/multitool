pub use backend::Backend;

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

pub struct State;
