use miette::Diagnostic;
use miette::IntoDiagnostic;
use miette::Result;
use std::path::Path;
use std::path::PathBuf;
use thiserror::Error;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

pub struct LambdaZip(Vec<u8>);

impl LambdaZip {
    pub async fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut bytes = Vec::new();
        let artifact_path = path.as_ref().to_path_buf();
        let file_result = File::open(&artifact_path).await;
        if file_result
            .as_ref()
            .is_err_and(|err| matches!(err.kind(), std::io::ErrorKind::NotFound))
        {
            return Err(MissingFileError {
                file: artifact_path.clone(),
            }
            .into());
        }
        let mut artifact = file_result.into_diagnostic()?;
        artifact.read_to_end(&mut bytes).await.into_diagnostic()?;
        Ok(Self(bytes))
    }

    /// Create an empty zip for tests.
    #[cfg(test)]
    pub fn mock() -> Self {
        Self(Vec::default())
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error("The path provided to the Lambda zip file does not exist.")]
#[diagnostic(help("Did you mean to provide this path?"))]
struct MissingFileError {
    file: PathBuf,
}

impl AsRef<[u8]> for LambdaZip {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
