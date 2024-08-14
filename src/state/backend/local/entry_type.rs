use serde::Serialize;

use crate::state::{backend::meta::PlanMetadata, ResourcePrototype, ResourceRecord};

/// We use a `EntryType` trait instead of an enum for forward compatibility.
/// Adding a new `Change` type works by implementing the trait.
pub trait EntryType {
    /// The `Resource` type refers to the data type contained
    /// within the change's body. This is typically either a
    /// `ResourceRecord` or a `ResourcePrototype`.
    type Resource: Serialize;
    fn operation() -> String;
}

pub trait NamedEntryType {
    const OPERATION: &'static str;
    type Resource: Serialize;
}

impl<T: NamedEntryType> EntryType for T {
    fn operation() -> String {
        Self::OPERATION.to_owned()
    }

    type Resource = T::Resource;
}

pub(super) struct Initialize;
impl NamedEntryType for Initialize {
    const OPERATION: &'static str = "INITIALIZE";
    type Resource = PlanMetadata;
}

pub(super) struct Finalize;
impl NamedEntryType for Finalize {
    const OPERATION: &'static str = "FINALIZE";
    type Resource = ();
}

pub(super) struct BeforeCreate;
impl NamedEntryType for BeforeCreate {
    const OPERATION: &'static str = "BEFORE CREATE";
    type Resource = ResourcePrototype;
}

pub(super) struct ProcessingCreate;
impl NamedEntryType for ProcessingCreate {
    const OPERATION: &'static str = "PROCESSING CREATE";
    type Resource = ResourcePrototype;
}

pub(super) struct AfterCreate;
impl NamedEntryType for AfterCreate {
    const OPERATION: &'static str = "AFTER CREATE";
    type Resource = ResourceRecord;
}

pub(super) struct BeforeUpdate;
impl NamedEntryType for BeforeUpdate {
    const OPERATION: &'static str = "BEFORE UPDATE";
    type Resource = ResourceRecord;
}

pub(super) struct ProcessingUpdate;
impl NamedEntryType for ProcessingUpdate {
    const OPERATION: &'static str = "PROCESSING UPDATE";
    type Resource = ResourceRecord;
}

pub(super) struct AfterUpdate;
impl NamedEntryType for AfterUpdate {
    const OPERATION: &'static str = "AFTER UPDATE";
    type Resource = ResourceRecord;
}

pub(super) struct BeforeDelete;
impl NamedEntryType for BeforeDelete {
    const OPERATION: &'static str = "BEFORE DELETE";
    type Resource = ResourceRecord;
}

pub(super) struct ProcessingDelete;
impl NamedEntryType for ProcessingDelete {
    const OPERATION: &'static str = "PROCESSING DELETE";
    type Resource = ResourceRecord;
}

pub(super) struct AfterDelete;
impl NamedEntryType for AfterDelete {
    const OPERATION: &'static str = "AFTER DELETE";
    type Resource = ResourceRecord;
}
