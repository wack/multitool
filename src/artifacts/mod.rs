use miette::IntoDiagnostic;
use miette::Result;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

pub struct LambdaZip(Vec<u8>);

impl LambdaZip {
    pub async fn load<P: AsRef<Path>>(artifact_path: P) -> Result<Self> {
        let mut bytes = Vec::new();
        let mut artifact = File::open(artifact_path).await.into_diagnostic()?;
        artifact.read_to_end(&mut bytes).await.into_diagnostic()?;
        Ok(Self(bytes))
    }

    /// Create an empty zip for tests.
    #[cfg(test)]
    pub fn mock() -> Self {
        Self(Vec::default())
    }
}

impl AsRef<[u8]> for LambdaZip {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
