use miette::Result;

use crate::Terminal;

pub struct Init {
    terminal: Terminal,
}

impl Init {
    pub fn new(terminal: Terminal) -> Self {
        Self { terminal }
    }

    pub fn dispatch(self) -> Result<()> {
        println!("Hello, world!");
        Ok(())
    }
}
