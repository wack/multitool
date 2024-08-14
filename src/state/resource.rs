use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A `ResourcePrototype` is the definition of a resource before
/// its created. It contains all of the required inputs to construct
/// the resource, but it can't know runtimes qualities of the resource
/// that are determined after the resource is created. For example,
/// a VM prototype might have a name, but it doesn't have an IP address,
/// because the IP address can only be known once the VM has been
/// created and assigned a network address.
#[derive(Serialize, Deserialize)]
pub struct ResourcePrototype {
    /// The prototype's unique identifier within the context of
    /// multitool. This field has no correspondance to state witin
    /// the cloud provider. It's used by multitool to construct a
    /// timeline of this resource's lifecycle.
    id: Uuid,
    inputs: serde_json::Value,
}

impl ResourcePrototype {
    pub fn new(id: Uuid, inputs: serde_json::Value) -> Self {
        Self { id, inputs }
    }
}

/// A ResourceRecord is a concrete resource within a cloud provider.
/// It corresponds to a resource that currenly exists.
#[derive(Serialize, Deserialize)]
pub struct ResourceRecord {
    /// The prototype's unique identifier within the context of
    /// multitool. This field has no correspondance to state witin
    /// the cloud provider. It's used by multitool to construct a
    /// timeline of this resource's lifecycle.
    id: Uuid,
    fields: serde_json::Value,
}

impl ResourceRecord {
    pub fn new(id: Uuid, fields: serde_json::Value) -> Self {
        Self { id, fields }
    }
}
