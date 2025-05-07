use clap::Subcommand;

// TODO: allow the server to be configured, e.g.
//       allow the user to pick the port.
#[derive(Subcommand, Clone)]
pub enum McpSubcommand {
    /// Run a Model-Context Protocol server.
    Server,
}
