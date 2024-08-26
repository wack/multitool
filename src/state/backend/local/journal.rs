use async_trait::async_trait;
use serde::Serialize;
use tokio::{fs::File, io::AsyncWriteExt};

use crate::state::{
    backend::meta::PlanMetadata, history::RunHistory, journal::Journal, ResourcePrototype,
    ResourceRecord,
};

use super::{
    entry::JournalEntry,
    entry_type::{
        AfterCreate, AfterDelete, AfterUpdate, BeforeCreate, BeforeDelete, BeforeUpdate, Finalize,
        Initialize, ProcessingCreate, ProcessingDelete, ProcessingUpdate,
    },
    errors::LocalError,
};

/// A `LocalJournal` writes journal entries to the local filesystem.
pub struct LocalJournal {
    file: File,
}

impl LocalJournal {
    /// We shouldn't accept a file directory. We should probably accept
    /// a Filesystem or a Wackfile or operate on a higher level
    /// of abstraction, like acceepting a trait here.
    pub fn new(file: File) -> Self {
        Self { file }
    }

    /// Writes a blob to the file and flushes it.
    async fn write_blob<T: Serialize>(&mut self, blob: &T) -> Result<(), std::io::Error> {
        // • Serialize the record into a string.
        let json = serde_json::to_vec(blob)?;
        // • Write the string to the file.
        self.file.write_all(&json).await?;
        // • Flush the contents of the buffer.
        self.file.flush().await?;
        Ok(())
    }
}

#[async_trait]
impl Journal for LocalJournal {
    type Error = LocalError;

    async fn before_create(&mut self, prototype: &ResourcePrototype) -> Result<(), Self::Error> {
        // • Convert the prototype into a JSON blob and write it to disk.
        let entry = JournalEntry::<BeforeCreate>::new(prototype);
        self.write_blob(&entry).await?;
        Ok(())
    }

    async fn after_create(&mut self, record: &ResourceRecord) -> Result<(), Self::Error> {
        // • Convert the record into a JSON blob and write it to disk.
        let entry = JournalEntry::<AfterCreate>::new(record);
        self.write_blob(&entry).await?;
        Ok(())
    }

    async fn before_delete(&mut self, record: &ResourceRecord) -> Result<(), Self::Error> {
        // • Convert the record into a JSON blob and write it to disk.
        let entry = JournalEntry::<BeforeDelete>::new(record);
        self.write_blob(&entry).await?;
        Ok(())
    }

    async fn after_delete(&mut self, record: &ResourceRecord) -> Result<(), Self::Error> {
        // • Convert the record into a JSON blob and write it to disk.
        let entry = JournalEntry::<AfterDelete>::new(record);
        self.write_blob(&entry).await?;
        Ok(())
    }

    async fn before_update(&mut self, record: &ResourceRecord) -> Result<(), Self::Error> {
        // • Convert the record into a JSON blob and write it to disk.
        let entry = JournalEntry::<BeforeUpdate>::new(record);
        self.write_blob(&entry).await?;
        Ok(())
    }

    async fn after_update(&mut self, record: &ResourceRecord) -> Result<(), Self::Error> {
        // • Convert the record into a JSON blob and write it to disk.
        let entry = JournalEntry::<AfterUpdate>::new(record);
        self.write_blob(&entry).await?;
        Ok(())
    }

    async fn create_processing(
        &mut self,
        prototype: &ResourcePrototype,
    ) -> Result<(), Self::Error> {
        // • Convert the prototype into a JSON blob and write it to disk.
        let entry = JournalEntry::<ProcessingCreate>::new(prototype);
        self.write_blob(&entry).await?;
        Ok(())
    }

    async fn delete_processing(&mut self, record: &ResourceRecord) -> Result<(), Self::Error> {
        // • Convert the record into a JSON blob and write it to disk.
        let entry = JournalEntry::<ProcessingDelete>::new(record);
        self.write_blob(&entry).await?;
        Ok(())
    }

    async fn update_processing(&mut self, record: &ResourceRecord) -> Result<(), Self::Error> {
        // • Convert the record into a JSON blob and write it to disk.
        let entry = JournalEntry::<ProcessingUpdate>::new(record);
        self.write_blob(&entry).await?;
        Ok(())
    }

    // TODO: This function has been moved into the `create_journal`
    // function on the Backend trait. When we open the file, we must
    // write the first entry to it at that time.
    /*
    async fn initialize(&mut self, plan: &PlanMetadata) -> Result<(), Self::Error> {
        let entry = JournalEntry::<Initialize>::new(plan);
        self.write_blob(&entry).await?;
        Ok(())
    }
    */

    async fn finalize(mut self) -> Result<RunHistory, Self::Error> {
        let entry = JournalEntry::<Finalize>::new(&());
        self.write_blob(&entry).await?;
        todo!("Have not implemented the RunHistory type yet");
    }
}
