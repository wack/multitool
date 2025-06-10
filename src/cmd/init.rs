//! The `init` subcommand is implemented as a state machine
//! that maniputates stack allocated data with each state.
//! The input is a (potentially empty) manifest file. Each state
//! asks a question of the user to fill in the manifest file,
//! like 20 Questions. As you answer more and more questions, you
//! navigate deeper and deeper into the manifest's structure,
//! until you reach a leaf node. Each leaf node corresponds to a
//! field in the manifest.

use miette::Result;

use crate::{Terminal, manifest::Manifest};

pub struct Init {
    terminal: Terminal,
}

impl Init {
    pub fn new(terminal: Terminal) -> Self {
        Self { terminal }
    }

    pub fn dispatch(self) -> Result<()> {
        // Create a new manifest instance.
        // TODO: In the future, we should load this manifest
        // from filesystem. See the git branch `robbie/init`
        // for the code.
        let mut manifest = Manifest::default();
        let start: Box<dyn StateMachine> = Box::new(Start(&mut manifest));
        let mut state = Some(start);
        while let Some(next) = state {
            state = next.run()?;
        }

        Ok(())
    }
}

struct Start<'a>(&'a mut Manifest);

impl<'a> StateMachine for Start<'a> {
    fn run(self: Box<Self>) -> Result<Option<Box<dyn StateMachine>>> {
        println!("Running once.");
        Ok(None)
    }
}

trait StateMachine {
    fn run(self: Box<Self>) -> Result<Option<Box<dyn StateMachine>>>;
}

#[cfg(test)]
mod tests {
    use super::StateMachine;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(StateMachine);
}
