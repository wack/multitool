#[derive(thiserror::Error, Debug, PartialEq, Eq)]
#[error("The value provided is outside of the inclusive range [0…100]")]
pub struct OutOfRangeError;
