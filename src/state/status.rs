use serde::{Deserialize, Serialize};

use super::resource::{ResourcePrototype, ResourceRecord};

#[derive(Serialize, Deserialize)]
pub enum Status {
    Began(ResourcePrototype),
    Pending,
    Success(ResourceRecord),
    Failure(String),
}
