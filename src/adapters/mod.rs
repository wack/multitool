pub use backend::{ApplicationConfig, BackendClient};
pub(crate) use backend::{ApplicationId, DeploymentId, DeploymentMetadata, StateId, WorkspaceId};

pub use ingresses::*;
pub use monitors::*;
pub use platforms::*;

mod backend;
/// Contains the trait definition and ingress implementations. Ingresses are responsible
/// for actuating changes to traffic.
mod ingresses;
/// Contains the trait definition for gathering monitoring data.
mod monitors;
mod platforms;
