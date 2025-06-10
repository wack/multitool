//! The `init` subcommand is implemented as a state machine
//! that maniputates stack allocated data with each state.
//! The input is a (potentially empty) manifest file. Each state
//! asks a question of the user to fill in the manifest file,
//! like 20 Questions. As you answer more and more questions, you
//! navigate deeper and deeper into the manifest's structure,
//! until you reach a leaf node. Each leaf node corresponds to a
//! field in the manifest.

use bon::Builder;
use miette::Result;

use crate::{Terminal, manifest::Manifest};

pub struct Init {
    terminal: Terminal,
}

impl Init {
    pub fn new(terminal: Terminal) -> Self {
        Self { terminal }
    }

    pub fn dispatch(mut self) -> Result<()> {
        // Create a new manifest instance.
        // TODO: In the future, we should load this manifest
        // from filesystem. See the git branch `robbie/init`
        // for the code.
        let mut manifest = Manifest::default();
        // Build the start state, passing in the terminal and manifest.
        let start: &mut dyn StateMachine = &mut Start::builder()
            .manifest(&mut manifest)
            .terminal(&mut self.terminal)
            .build();
        // Run the state machine to completion.
        let mut state = Some(start);
        while let Some(next) = state {
            state = next.run()?;
        }

        Ok(())
    }
}

#[derive(Builder)]
struct Start<'a> {
    manifest: &'a mut Manifest,
    terminal: &'a mut Terminal,
}

impl StateMachine for Start<'_> {
    fn run(&mut self) -> Result<Option<&mut dyn StateMachine>> {
        println!("Running once.");
        let next = Self::builder()
            .manifest(&mut self.manifest)
            .terminal(&mut self.terminal)
            .build();
        Ok(None)
    }
}

trait StateMachine {
    fn run(&mut self) -> Result<Option<&mut dyn StateMachine>>;
}

#[cfg(test)]
mod tests {
    use super::StateMachine;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(StateMachine);
}
