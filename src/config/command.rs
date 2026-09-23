use clap::Subcommand;
use miette::Result;

#[cfg(feature = "jev")]
use crate::cmd::Plan;
#[cfg(feature = "proxy")]
use crate::cmd::Proxy;
use crate::cmd::{Check, Init, Login, Logout, Run, Version};
use crate::terminal::Terminal;

use super::{CheckSubcommand, InitSubcommand, LoginSubcommand, RunSubcommand};

#[cfg(feature = "jev")]
use super::PlanSubcommand;
#[cfg(feature = "proxy")]
use super::ProxySubcommand;

/// A `MultiCommand` is one of the top-level commands accepted by
/// the multi CLI.
///
/// The repository now hosts MultiTool Checks, whose only relevant subcommand is
/// `check`. The remaining subcommands are leftovers from the tool's previous
/// life and are soft-deprecated (MULTI-1365): they still function but are hidden
/// from help and emit a deprecation notice when invoked. They will be removed in
/// a future release (MULTI-1366). Standard utility commands (`check`, `version`,
/// and clap's built-in `help`) are preserved and never deprecated.
#[derive(Subcommand, Clone)]
pub enum MultiCommand {
    /// Log in to the hosted SaaS.
    #[command(hide = true)]
    Login(LoginSubcommand),
    #[command(hide = true)]
    Logout,
    /// Initialize a new application or prepare the configuration of an existing one
    #[command(hide = true)]
    Init(InitSubcommand),
    #[cfg(feature = "proxy")]
    #[command(hide = true)]
    Proxy(ProxySubcommand),
    /// Run will execute `multi` in "runner mode", where it will
    /// immediately deploy the provided artifact and start canarying.
    #[command(hide = true)]
    Run(RunSubcommand),
    /// Validate the requirements declared in `CHECKS.md` files using AI-agent checks.
    Check(CheckSubcommand),
    /// Establish what evidence is necessary to verify each check and freeze it
    /// into `.check-plan.toml` (the Jev decision engine).
    #[cfg(feature = "jev")]
    Plan(PlanSubcommand),
    /// Print the CLI version and exit
    Version,
}

impl MultiCommand {
    /// The user-facing name of this subcommand if it is a soft-deprecated legacy
    /// command, or `None` for supported commands (`check`, `version`).
    ///
    /// Used to emit a deprecation notice on dispatch. Kept exhaustive (no `_`
    /// arm) so adding a new subcommand forces a deliberate keep-vs-deprecate
    /// decision here.
    fn deprecated_name(&self) -> Option<&'static str> {
        match self {
            Self::Login(_) => Some("login"),
            Self::Logout => Some("logout"),
            Self::Init(_) => Some("init"),
            #[cfg(feature = "proxy")]
            Self::Proxy(_) => Some("proxy"),
            Self::Run(_) => Some("run"),
            #[cfg(feature = "jev")]
            Self::Plan(_) => None,
            Self::Check(_) | Self::Version => None,
        }
    }

    /// dispatch the user-provided arguments to the command handler.
    pub fn dispatch(self, console: Terminal) -> Result<()> {
        if let Some(name) = self.deprecated_name() {
            console.deprecation_notice(name)?;
        }
        match self {
            Self::Login(flags) => Login::new(console, flags)?.dispatch(),
            Self::Logout => Logout::new(console).dispatch(),
            Self::Init(flags) => Init::new(console, flags)?.dispatch(),
            #[cfg(feature = "proxy")]
            Self::Proxy(flags) => Proxy::new(console, flags).dispatch(),
            Self::Run(flags) => Run::new(console, flags)?.dispatch(),
            Self::Check(flags) => Check::new(console, flags)?.dispatch(),
            #[cfg(feature = "jev")]
            Self::Plan(flags) => Plan::new(console, flags)?.dispatch(),
            Self::Version => Version::new(console).dispatch(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use clap::{CommandFactory, Parser};

    use super::MultiCommand;
    use crate::Cli;

    /// Parse a full CLI invocation and return the selected subcommand.
    fn command_of(args: &[&str]) -> MultiCommand {
        Cli::parse_from(args)
            .cmd()
            .clone()
            .expect("a subcommand was provided")
    }

    #[test]
    fn legacy_commands_report_their_deprecated_name() {
        assert_eq!(
            command_of(&["multi", "login"]).deprecated_name(),
            Some("login")
        );
        assert_eq!(
            command_of(&["multi", "logout"]).deprecated_name(),
            Some("logout")
        );
        assert_eq!(
            command_of(&["multi", "init"]).deprecated_name(),
            Some("init")
        );
        assert_eq!(command_of(&["multi", "run"]).deprecated_name(), Some("run"));
    }

    #[test]
    fn supported_commands_are_not_deprecated() {
        assert_eq!(command_of(&["multi", "check"]).deprecated_name(), None);
        assert_eq!(command_of(&["multi", "version"]).deprecated_name(), None);
    }

    #[test]
    fn legacy_commands_are_hidden_and_supported_commands_are_visible() {
        let hidden: HashMap<String, bool> = Cli::command()
            .get_subcommands()
            .map(|c| (c.get_name().to_string(), c.is_hide_set()))
            .collect();

        for name in ["login", "logout", "init", "run"] {
            assert!(
                hidden[name],
                "legacy `{name}` should be hidden from help output"
            );
        }
        // `help` is clap's built-in utility; `check`/`version` are supported.
        for name in ["check", "version"] {
            assert!(
                !hidden[name],
                "`{name}` should remain advertised in help output"
            );
        }
    }

    /// MULTI-1824 acceptance: `multi plan` does not exist in a default-feature
    /// build. Parsing must fail outright (unknown subcommand), and the
    /// subcommand must be absent from the list `--help` renders — the latter
    /// is the regression guard for "default-build `--help` output is
    /// byte-for-byte unchanged": a `Plan` variant that leaked outside its
    /// `#[cfg(feature = "jev")]` gate would show up here.
    #[cfg(not(feature = "jev"))]
    #[test]
    fn plan_subcommand_does_not_exist_without_the_jev_feature() {
        // `Cli` (clap's `Parser::try_parse_from` success type) isn't `Debug`,
        // so `expect_err`/`unwrap_err` (which require it for their panic
        // message) don't apply here — match manually instead.
        let result = Cli::try_parse_from(["multi", "plan"]);
        let err = match result {
            Err(err) => err,
            Ok(_) => panic!("plan must be unknown in a default-feature build"),
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);

        let names: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        assert!(!names.contains(&"plan".to_string()), "found: {names:?}");
    }

    /// The `--features jev` counterpart: `multi plan` parses, is not
    /// deprecated, and is advertised (not hidden) in help output — the same
    /// treatment as `check`/`version`.
    #[cfg(feature = "jev")]
    #[test]
    fn plan_subcommand_parses_and_is_visible_under_the_jev_feature() {
        assert_eq!(command_of(&["multi", "plan"]).deprecated_name(), None);

        let hidden = Cli::command()
            .get_subcommands()
            .find(|c| c.get_name() == "plan")
            .map(|c| c.is_hide_set());
        assert_eq!(hidden, Some(false));
    }
}
