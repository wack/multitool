use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Diagnostic, Error)]
pub enum LocalError {
    #[error(transparent)]
    Io(std::io::Error),
}

impl From<std::io::Error> for LocalError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}
