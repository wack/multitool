use serde::{Deserialize, Serialize};

// TODO: Add a way to extract the lease duration from the protocol
//       version.

/// `Protocol` describes the protocol for exchanging data with the backend
/// At this time, the only value contained is the protocol version, which
/// defined the amount of time a lease lasts for.
#[derive(Default, PartialEq, Eq, Serialize, Deserialize, Debug, Copy, Clone)]
pub struct Protocol {
    version: ProtocolVersion,
}

#[derive(Default, PartialEq, Eq, Serialize, Deserialize, Debug, Copy, Clone)]
pub enum ProtocolVersion {
    #[default]
    V0,
}
