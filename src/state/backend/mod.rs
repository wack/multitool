use super::journal::Journal;
use super::project::Project;
use super::State;
use async_trait::async_trait;

pub use local::LocalBackend;

/// A `Backend` is responsible for making updates to state durable.
/// Backends can read and write state.
#[async_trait]
pub trait Backend {
    type J: Journal;
    type Error: Into<miette::Error>;

    /// atomically load the state from the backend. This function
    /// can block if the state is locked.
    async fn fetch_state(&self, proj: &Project) -> Result<State, Self::Error>;

    /// save the state to the backend and associate it with this project.
    async fn persist(&mut self, proj: &Project, state: &State) -> Result<(), Self::Error>;

    /// create a new journal. If one already exists, return an error.
    async fn new_journal(&mut self, proj: &Project) -> Result<Self::J, Self::Error>;
}

mod local;
mod meta;
mod protocol;

#[cfg(test)]
mod tests {
    use super::local::LocalJournal;
    use super::Backend;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Backend<J = LocalJournal, Error = ()>);
}
