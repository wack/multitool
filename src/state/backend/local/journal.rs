use std::pin::Pin;

use async_trait::async_trait;
use serde::Serialize;
use tokio::{
    fs::File,
    io::{AsyncBufRead, AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt},
};

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

pub(super) trait LocalStore: AsyncWrite + AsyncBufRead + AsyncSeek + Send + 'static {}
impl<T: AsyncWrite + AsyncBufRead + AsyncSeek + Send + 'static> LocalStore for T {}

pub struct LocalJournal {
    store: Pin<Box<dyn LocalStore>>,
}

impl LocalJournal {
    pub(super) fn new(store: impl LocalStore) -> Self {
        Self {
            store: Box::pin(store),
        }
    }

    /// Writes a blob to the store
    async fn write_blob<T: Serialize>(&mut self, blob: &T) -> Result<(), std::io::Error> {
        // Serialize the record into a string
        let json = serde_json::to_vec(blob)?;
        // Write the string to the store
        self.store.write_all(&json).await?;
        // Flush the contents of the buffer
        self.store.flush().await?;
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

#[cfg(test)]
mod tests {

    use std::io::Cursor;

    use miette::{IntoDiagnostic, Result};
    use serde_json::json;
    use uuid::Uuid;

    use crate::state::ResourcePrototype;
    use crate::state::ResourceRecord;

    use super::Journal;
    use super::LocalJournal;

    #[tokio::test]
    async fn in_memory_journal_write() -> Result<()> {
        let store = Cursor::new(Vec::new());
        let mut journal = LocalJournal::new(store);

        let (proto, record) = fake_resource();

        journal.before_create(&proto).await.into_diagnostic()?;
        // End with a creation complete operation.
        journal.after_create(&record).await.into_diagnostic()?;
        // journal.finalize().await.into_diagnostic()?; // TODO: needs to be implemented

        Ok(())
    }

    fn fake_resource() -> (ResourcePrototype, ResourceRecord) {
        let id = Uuid::new_v4();

        let proto = ResourcePrototype::new(
            id,
            json!({
                "hello": "world",
            }),
        );

        let record = ResourceRecord::new(
            &proto,
            json!({
                "hello": "world",
                "multitool": "rules",
            }),
        );
        (proto, record)
    }
}
