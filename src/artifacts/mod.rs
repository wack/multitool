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
}
