use std::path::PathBuf;

use crate::{artifacts::LambdaZip, config::RunSubcommand};
use miette::Result;

use crate::Terminal;

/// Deploy the Lambda function as a canary and monitor it.
pub struct Run {
    terminal: Terminal,
    artifact_path: PathBuf,
    // TODO: We probably will need to know the desired
    //       workspace and application before we can
    //       create the new deployment.
    // workspace: String,
    // application: String,
}

impl Run {
    pub fn new(terminal: Terminal, args: RunSubcommand) -> Self {
        Self {
            terminal,
            artifact_path: args.artifact_path().clone(),
        }
    }

    pub async fn dispatch(self) -> Result<()> {
        // • First, we have to load the artifact.
        //   This lets us fail fast in the case where the artifact
        //   doesn't exist or we don't have permission to read the file.
        let artifact = LambdaZip::load(self.artifact_path).await?;
        // • Now, we have to load the application's configuration
        //   from the backend. We have the name of the workspace and
        //   application, but we need to look up the details.
        todo!();
    }
}
