pub use backend::{ApplicationConfig, BackendClient};
pub(crate) use backend::{LockedState, RolloutMetadata};
pub use cloudflare::CloudFlareClient;

pub use ingresses::*;
pub use monitors::*;
pub use platforms::*;

pub mod backend;
/// MultiTool's CloudFlare HTTP client.
mod cloudflare;
/// Contains the trait definition and ingress implementations. Ingresses are responsible
/// for actuating changes to traffic.
mod ingresses;
/// Contains the trait definition for gathering monitoring data.
mod monitors;
mod platforms;
