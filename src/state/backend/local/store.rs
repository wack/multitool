// NOTE: Unused, but want an explanation of why this doesn't work...

use std::{
    io::{Cursor, SeekFrom},
    task::{Context, Poll},
};

use serde::Serialize;
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt},
};

use std::pin::Pin;

pub trait LocalStore: AsyncWrite + AsyncRead + AsyncSeek {
    fn new<T: AsyncWrite + AsyncRead + AsyncSeek>(store: &T) -> Self;
    fn write<T: Serialize>(&self, blob: &T) -> Result<(), std::io::Error>;
    fn read(&self) -> Result<(), std::io::Error>;
}

pub struct FileStore {
    file: File,
}

// Trait bounds are not implemented for this type
impl LocalStore for FileStore {
    fn new(file: File) -> Self {
        Self { file }
    }

    async fn write<T: Serialize>(&mut self, blob: &T) -> Result<(), std::io::Error> {
        // Serialize the record into a string
        let json = serde_json::to_vec(blob)?;
        // Write the string to the file
        self.file.write_all(&json).await?;
        // Flush the contents of the buffer
        self.file.flush().await?;
        Ok(())
    }

    fn read(&self) -> Result<(), std::io::Error> {
        todo!();
        Ok(())
    }
}

pub struct MemoryStore {
    cursor: Cursor<Vec<u8>>,
}

// Trait bounds are not implemented for this type
impl LocalStore for MemoryStore {
    fn new(cursor: Cursor<Vec<u8>>) -> Self {
        Self { cursor }
    }

    async fn write<T: Serialize>(&mut self, blob: &T) -> Result<(), std::io::Error> {
        // Serialize the record into a string
        let json = serde_json::to_vec(blob)?;
        // Write the string to the in-memory obj
        self.cursor.write_all(&json).await?;
        Ok(())
    }

    // TODO: Implement this and have it return real data
    fn read(&self) -> Result<(), std::io::Error> {
        todo!();
        Ok(())
    }
}
