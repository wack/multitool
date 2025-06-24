use std::fmt;

use clap::Args;

const CONTROLLER_NAME: &str = "Controller";
const API_SERVER_NAME: &str = "API Server";

#[derive(Debug, Clone, PartialEq)]
pub enum GatewayMode {
    Controller,
    ApiServer,
}

impl fmt::Display for GatewayMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GatewayMode::Controller => write!(f, "{}", CONTROLLER_NAME),
            GatewayMode::ApiServer => write!(f, "{}", API_SERVER_NAME),
        }
    }
}

impl std::str::FromStr for GatewayMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            CONTROLLER_NAME => Ok(GatewayMode::Controller),
            API_SERVER_NAME => Ok(GatewayMode::ApiServer),
            _ => Err(format!("Invalid gateway mode: {}", s)),
        }
    }
}

#[derive(Args, Clone)]
pub struct GatewaySubcommand {
    /// The mode to run the gateway in
    #[arg(long, short = 'm', default_value_t = GatewayMode::ApiServer)]
    mode: GatewayMode,
}

impl GatewaySubcommand {
    pub fn mode(&self) -> &GatewayMode {
        &self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_mode_display() {
        assert_eq!(GatewayMode::Controller.to_string(), CONTROLLER_NAME);
        assert_eq!(GatewayMode::ApiServer.to_string(), API_SERVER_NAME);
    }
}