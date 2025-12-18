pub use backend::BackendClient;
pub(crate) use backend::{LockedState, RolloutMetadata};
pub use cloudflare::CloudflareClient;
#[cfg(feature = "vercel")]
pub use vercel::VercelClient;

pub use ingresses::*;
pub use monitors::*;
pub use platforms::*;

pub mod backend;
/// MultiTool's Cloudflare HTTP client.
mod cloudflare;
/// MultiTool's Vercel HTTP client.
#[cfg(feature = "vercel")]
mod vercel;
/// Contains the trait definition and ingress implementations. Ingresses are responsible
/// for actuating changes to traffic.
mod ingresses;
/// Contains the trait definition for gathering monitoring data.
mod monitors;
/// Contains the trait definition and platform implementations.
mod platforms;
