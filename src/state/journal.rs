use async_trait::async_trait;

use super::{
    history::RunHistory,
    resource::{ResourcePrototype, ResourceRecord},
};

#[async_trait]
pub trait Journal {
    type Error: Into<miette::Error>;

    /// finalize writes the last entry to the journal (the entry that signifies
    /// the end of the run) and converts the journal into a RunHistory,
    /// removing unnecessary operations.
    async fn finalize(self) -> Result<RunHistory, Self::Error>;

    /// Immediately before creating a resource, journal the change to a destination
    /// (a file for local development, or an HTTP API for remote development).
    ///
    /// NOTE: This function must be called and awaited immediately
    /// before a resource is created.
    async fn before_create(&mut self, _: &ResourcePrototype) -> Result<(), Self::Error>;

    async fn create_processing(&mut self, _: &ResourcePrototype) -> Result<(), Self::Error>;

    /// Immediately after creating a resource, journal a change to a destination
    /// (a file for local development, or an HTTP API for remote development).
    ///
    /// NOTE: This function must be called and awaited immediately
    /// before a resource is created.
    async fn after_create(&mut self, _: &ResourceRecord) -> Result<(), Self::Error>;

    /// Immediately before deleting a resource, journal a change to a destination
    /// (a file for local development, or an HTTP API for remote development).
    ///
    /// NOTE: This function must be called and awaited immediately
    /// before a resource is created.
    async fn before_delete(&mut self, _: &ResourceRecord) -> Result<(), Self::Error>;
    async fn delete_processing(&mut self, _: &ResourceRecord) -> Result<(), Self::Error>;
    /// Immediately after deleting a resource, journal a change to a destination
    /// (a file for local development, or an HTTP API for remote development).
    ///
    /// NOTE: This function must be called and awaited immediately
    /// before a resource is created.
    async fn after_delete(&mut self, _: &ResourceRecord) -> Result<(), Self::Error>;

    /// Immediately before updating a resource, journal a change to a destination
    /// (a file for local development, or an HTTP API for remote development).
    ///
    /// NOTE: This function must be called and awaited immediately
    /// before a resource is created.
    async fn before_update(&mut self, _: &ResourceRecord) -> Result<(), Self::Error>;
    async fn update_processing(&mut self, _: &ResourceRecord) -> Result<(), Self::Error>;

    /// Immediately after updating a resource, journal a change to a destination
    /// (a file for local development, or an HTTP API for remote development).
    ///
    /// NOTE: This function must be called and awaited immediately
    /// before a resource is created.
    async fn after_update(&mut self, _: &ResourceRecord) -> Result<(), Self::Error>;
}
