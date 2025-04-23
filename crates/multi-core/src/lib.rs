use miette::Diagnostic;
use thiserror::Error;

/// Utilities for reading and manipulating files.
pub mod fs;
/// Utilities for hashing files and strings.
pub mod hashing;

/// This type aggregates multiple errors into a single instance.
#[derive(Debug, Error, Diagnostic, Default)]
#[error("The following errors occurred during execution")]
pub struct ManyError {
    #[related]
    collection: Vec<miette::Error>,
}

impl ManyError {
    pub fn append(&mut self, err: miette::Error) {
        self.collection.push(err);
    }

    pub fn is_empty(&self) -> bool {
        self.collection.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tautology() {
        assert_eq!(1 + 1, 2);
    }
}
