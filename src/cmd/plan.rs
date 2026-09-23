use miette::{IntoDiagnostic, Result};
use tokio::runtime::Runtime;

use crate::Terminal;
use crate::config::PlanSubcommand;

/// The `multi plan` command handler (the Jev decision engine, MULTI-1824).
/// Mirrors `Check`: a synchronous `dispatch()` that builds a `tokio` runtime
/// and `block_on`s the async pipeline.
pub struct Plan {
    terminal: Terminal,
    args: PlanSubcommand,
}

impl Plan {
    pub fn new(terminal: Terminal, args: PlanSubcommand) -> Result<Self> {
        Ok(Self { terminal, args })
    }

    pub fn dispatch(self) -> Result<()> {
        let rt = Runtime::new().into_diagnostic()?;
        let _guard = rt.enter();
        // Mirrors `Check::dispatch`: a plan that could not be fully written
        // exits non-zero cleanly (`Ok(1)`); an operational error (invalid
        // `CHECKS.md`, an aborting Jev credential failure) surfaces as a
        // `miette` diagnostic (`Err`) instead.
        let code = rt.block_on(crate::checks::plan::run(
            &self.terminal,
            self.args.directory(),
            self.args.overrides(),
            self.args.force(),
        ))?;
        std::process::exit(code);
    }
}
