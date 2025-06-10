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
        let start: Box<dyn InitState<Ctx = Manifest>> = Box::new(());
        let mut manifest = Manifest::default();
        let mut state = Some(start);
        while let Some(next) = state {
            state = next.run(&mut manifest)?;
        }

        Ok(())
    }
}

impl<T> InitState for T {
    type Ctx = Manifest;

    fn run(self, _: &mut Self::Ctx) -> Result<Option<Box<dyn InitState<Ctx = Self::Ctx>>>> {
        println!("Running once.");
        Ok(None)
    }
}

trait InitState {
    type Ctx;

    fn run(self, _: &mut Self::Ctx) -> Result<Option<Box<dyn InitState<Ctx = Self::Ctx>>>>;
}

#[cfg(test)]
mod tests {
    use crate::manifest::Manifest;

    use super::InitState;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(InitState<Ctx = Manifest>);
}
