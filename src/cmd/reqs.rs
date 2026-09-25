use miette::{IntoDiagnostic, Result, miette};
use reqs_cli::ReqsArgs;
use tokio::runtime::Runtime;

/// The `multi reqs` command handler: requirement AND-OR graphs over a Datalog
/// core, stored in SQLite (see `crates/reqs/README.md`). Like `Check`, a
/// synchronous `dispatch()` that builds a `tokio` runtime and `block_on`s the
/// async command in `reqs_cli`.
pub struct Reqs {
    args: ReqsArgs,
}

impl Reqs {
    pub fn new(args: ReqsArgs) -> Self {
        Self { args }
    }

    pub fn dispatch(self) -> Result<()> {
        let rt = Runtime::new().into_diagnostic()?;
        // `{:#}` keeps anyhow's context chain ("reading foo.dl: No such file").
        rt.block_on(reqs_cli::run(self.args))
            .map_err(|e| miette!("{e:#}"))
    }
}
