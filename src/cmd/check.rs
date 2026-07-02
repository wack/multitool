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
        // Exit directly rather than returning and letting `rt` drop: dropping a
        // `Runtime` blocks the calling thread until every task it ever spawned
        // has fully unwound (`Runtime::drop`'s docs: "The thread initiating the
        // shutdown blocks until all spawned work has been stopped. This can
        // take an indefinite amount of time."). By this point the check pipeline
        // has already produced its result and, for a TTY run, the presenter has
        // already flushed the terminal record — there is nothing further for
        // this process to do, so it should not be held hostage by a stray task
        // (ours or a dependency's) that is slow, or fails, to wind down. This
        // used to only apply on the `code != 0` path, which papered over the
        // asymmetry: an all-checks-pass run had no hard exit and could hang
        // instead of returning control to the shell.
        std::process::exit(code);
    }
}
