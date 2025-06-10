//! The `init` subcommand is implemented as a state machine
//! that maniputates stack allocated data with each state.
//! The input is a (potentially empty) manifest file. Each state
//! asks a question of the user to fill in the manifest file,
//! like 20 Questions. As you answer more and more questions, you
//! navigate deeper and deeper into the manifest's structure,
//! until you reach a leaf node. Each leaf node corresponds to a
//! field in the manifest.

use bon::Builder;
use miette::{IntoDiagnostic, Result};
use tokio::runtime::Runtime;
use tracing::info;

use crate::{
    Terminal,
    adapters::BackendClient,
    config::InitSubcommand,
    fs::FileSystem,
    manifest::{InitTomlManifest, Manifest, TomlManifest},
};

pub struct Init {
    terminal: Terminal,
    backend: BackendClient,
}

impl Init {
    pub fn new(terminal: Terminal, flags: InitSubcommand) -> Result<Self> {
        let origin = flags.origin().as_deref();
        let backend = BackendClient::new(origin, None)?;
        Ok(Self { terminal, backend })
    }

    pub fn dispatch(mut self) -> Result<()> {
        // Kick off the async runtime.
        let rt = Runtime::new().into_diagnostic()?;
        let _guard = rt.enter();
        rt.block_on(async {
            let mut fs = FileSystem::new()?;
            // Exit early if there's already a project manifest.
            //
            // For now, we don't allow users to use init to
            // edit their manifest (because we can't preserve
            // comments right Serde right now...)
            // TODO: Upgrade our config-file reading to use
            // the toml and toml_edit crates to preserve comments
            // in the TOML files we read.
            // https://docs.rs/toml_edit/latest/toml_edit/
            if let Ok(_) = fs.project_manifest() {
                info!("It looks like you already have an initialized project manifest. Exiting.");
                return Ok(());
            }
            info!("No project manifest file found. Let's create one!");
            // Create a new manifest instance.
            let mut manifest = Manifest::default();

            // Build the start state, passing in the terminal and manifest.
            let start: &mut dyn InitStateMachine = &mut Start::builder()
                .manifest(&mut manifest)
                .terminal(&mut self.terminal)
                .fs(&mut fs)
                .build();

            // Run the state machine to completion.
            let mut state = Some(start);
            while let Some(next) = state {
                state = next.run()?;
            }

            // Dump the file to disk.
            fs.save_file(&InitTomlManifest, &manifest)
        })
    }
}

#[derive(Builder)]
struct Start<'a> {
    manifest: &'a mut Manifest,
    terminal: &'a mut Terminal,
    fs: &'a mut FileSystem,
}

impl InitStateMachine for Start<'_> {
    fn run(&mut self) -> Result<Option<&mut dyn InitStateMachine>> {
        println!("Running once.");
        let next = Self::builder()
            .manifest(&mut self.manifest)
            .terminal(&mut self.terminal)
            .fs(self.fs)
            .build();
        Ok(None)
    }
}

trait InitStateMachine {
    fn run(&mut self) -> Result<Option<&mut dyn InitStateMachine>>;
}

#[cfg(test)]
mod tests {
    use super::InitStateMachine;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(InitStateMachine);
}
