use async_trait::async_trait;

use crate::state::{project::Project, Backend, State};

use super::{errors::LocalError, LocalJournal};

/// The Local backend uses the filesystem to store state.
pub struct LocalBackend;

#[async_trait]
impl Backend for LocalBackend {
    type J = LocalJournal;

    type Error = LocalError;

    async fn fetch_state(&self, _: &Project) -> Result<State, Self::Error> {
        todo!()
    }

    async fn persist(&mut self, _: &Project, _: &State) -> Result<(), Self::Error> {
        todo!()
    }

    async fn new_journal(&mut self, _: &Project) -> Result<Self::J, Self::Error> {
        todo!()
    }
}
