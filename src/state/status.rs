use miette::Error;

use super::resource::{ResourcePrototype, ResourceRecord};

pub enum Status {
    Began(ResourcePrototype),
    Pending,
    Success(ResourceRecord),
    Failure(Error),
}
