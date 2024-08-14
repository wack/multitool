use serde::{Deserialize, Serialize};

use super::entry_type::EntryType;

/// A `JournalEntry` defines a single row or record in the journal
/// used for change management. The `ChangeManager` makes one entry
/// for each change.
#[derive(Serialize, Deserialize)]
#[serde(from = "JournalEntry<T>")]
pub(super) struct JournalEntry<'a, T: EntryType> {
    pub data: &'a T::Resource,
}

impl<'a, T: EntryType> JournalEntry<'a, T> {
    pub(super) fn new(data: &'a T::Resource) -> Self {
        Self { data }
    }
}

/// This type is just an expanded form of `JournalEntry` used
/// to conventially marshal and unmarshal from a string. That's why
/// its private to this module.
#[derive(Serialize, Deserialize)]
struct Entry<T: EntryType> {
    data: T::Resource,
    operation: String,
}
