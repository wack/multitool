use miette::{IntoDiagnostic, Result};
use tokio::runtime::Runtime;

use crate::Terminal;
use crate::config::CheckSubcommand;

/// The `multi check` command handler. Mirrors `Run`/`Login`/`Init`: a
/// synchronous `dispatch()` that builds a `tokio` runtime and `block_on`s the
/// async pipeline. The runtime is established here because the MCP result server
/// (M4) must run on a task *within this process* (never a subprocess).
pub struct Check {
    terminal: Terminal,
    args: CheckSubcommand,
}

impl Check {
    pub fn new(terminal: Terminal, args: CheckSubcommand) -> Result<Self> {
        Ok(Self { terminal, args })
    }

    pub fn dispatch(self) -> Result<()> {
        let rt = Runtime::new().into_diagnostic()?;
        let _guard = rt.enter();
        // The pipeline returns a process exit code distinct from operational
        // errors: check *failures* exit 1 cleanly; invalid input surfaces as a
        // `miette` diagnostic (non-zero) instead.
        let code = rt.block_on(crate::checks::run(
            &self.terminal,
            self.args.directory(),
            self.args.overrides(),
        ))?;
        if code != 0 {
            std::process::exit(code);
        }
        Ok(())
    }
}
