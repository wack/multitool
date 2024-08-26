use serde::{Deserialize, Serialize};

use crate::state::project::Project;

use super::protocol::Protocol;

/// PlanMetadata provides extra information about the plan itself
/// known before its execution. For example, it describes the
/// project the plan is for as well as the protocol used for the plan.
#[derive(Serialize, Deserialize)]
pub struct PlanMetadata {
    /// TODO: We need some unique identifier for this particular
    ///       project. We can either use a natural key, like a
    ///       user-provided name, which might help on local systems,
    ///       or a Uuid, which might get confusing. This value
    ///       probably has to be configured by the user, so it will
    ///       probably sit in the user's config file. If this is the
    ///       case, then we shouldn't use a Uuid, and instead use
    ///       some natural key.
    project: Project,
    protocol: Protocol,
}

impl PlanMetadata {
    pub fn new(project: &Project) -> Self {
        Self {
            project: project.clone(),
            protocol: Protocol::default(),
        }
    }
}
