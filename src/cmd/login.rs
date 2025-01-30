use miette::Result;

use crate::Terminal;

/// Deploy the Lambda function as a canary and monitor it.
pub struct Login {
    terminal: Terminal,
}

impl Login {
    pub fn new(terminal: Terminal) -> Self {
        Self { terminal }
    }

    pub fn dispatch(self) -> Result<()> {
        todo!();
    }
}
