use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloudflareRoute {
    pub id: String,
    pub pattern: String,
    pub script: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCloudflareRouteRequest {
    pub pattern: String,
    pub script: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCloudflareRouteRequest {
    pub pattern: String,
    pub script: String,
}
