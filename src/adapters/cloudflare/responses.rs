use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloudflareError {
    pub code: i64,
    pub message: String,
    pub documentation_url: Option<String>,
    #[serde(default)]
    pub source: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloudflareMessage {
    pub code: i64,
    pub message: String,
    pub documentation_url: Option<String>,
    #[serde(default)]
    pub source: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloudflareResponse<T> {
    pub errors: Vec<CloudflareError>,
    pub messages: Vec<CloudflareMessage>,
    pub success: bool,
    pub result: T,
}
