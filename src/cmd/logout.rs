use miette::Result;

use crate::{
    Terminal,
    fs::{FileSystem, Session},
};

/// Deploy the Lambda function as a canary and monitor it.
pub struct Logout {
    terminal: Terminal,
}

impl Logout {
    pub fn new(terminal: Terminal) -> Self {
        Self { terminal }
    }

    /// Delete the user's session file if it exists.
    pub fn dispatch(self) -> Result<()> {
        let fs = FileSystem::new()?;
        // Delete the user's session file.
        // We don't care if the user was already logged in or not,
        // so we ignore the return type.
        let report = fs.delete_file::<Session>().map(|_| ());
        if report.is_ok() {
            self.terminal.logout_successful()?;
        }
        report
    }
}
