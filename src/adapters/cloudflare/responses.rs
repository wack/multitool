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
    pub code: Option<i64>,
    pub message: Option<String>,
    pub documentation_url: Option<String>,
    #[serde(default)]
    pub source: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CloudflareResponse<T> {
    pub errors: Option<Vec<CloudflareError>>,
    pub messages: Option<Vec<CloudflareMessage>>,
    pub success: bool,
    pub result: T,
}
