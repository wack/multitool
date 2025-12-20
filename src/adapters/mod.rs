pub use backend::{RolloutMetadata, BackendClient};
pub use cloudflare::CloudflareClient;

pub(crate) use backend::{LockedState};

pub use ingresses::*;
pub use monitors::*;
pub use platforms::*;

pub mod backend;
/// MultiTool's Cloudflare HTTP client.
mod cloudflare;
/// Contains the trait definition and ingress implementations. Ingresses are responsible
/// for actuating changes to traffic.
mod ingresses;
/// Contains the trait definition for gathering monitoring data.
mod monitors;
/// Contains the trait definition and platform implementations.
mod platforms;
