use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct MetricsResponse {
    #[serde(default)]
    pub calculations: Vec<Calculation>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Calculation {
    #[serde(default)]
    pub aggregates: Vec<Aggregate>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Aggregate {
    #[serde(default)]
    pub count: u32,
}

#[derive(Deserialize, Debug)]
pub struct ErrorLogsResponse {
    #[serde(default)]
    pub invocations: Vec<Vec<InvocationData>>,
}

#[derive(Deserialize, Debug)]
pub struct InvocationData {
    #[serde(rename = "$workers")]
    pub workers: WorkersData,
    pub source: SourceData,
}

#[derive(Deserialize, Debug)]
pub struct WorkersData {
    pub event: EventData,
}

#[derive(Deserialize, Debug)]
pub struct EventData {
    pub request: RequestData,
    pub response: ResponseData,
}

#[derive(Deserialize, Debug)]
pub struct RequestData {
    pub url: String,
    pub method: String,
    pub path: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ResponseData {
    pub status: u16,
}

#[derive(Deserialize, Debug)]
pub struct SourceData {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub exception: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CloudflareErrorLog {
    pub url: String,
    pub method: String,
    pub path: String,
    pub response: ResponseData,
    pub source: ErrorSource,
}

#[derive(Debug, Clone)]
pub struct ErrorSource {
    pub message: Option<String>,
    pub exception: Option<String>,
}

/// Represents a group of error logs from a single invocation
pub type CloudflareErrorLogGroup = Vec<CloudflareErrorLog>;

/// The complete structure maintaining invocation grouping: Vec<Vec<CloudflareErrorLog>>
pub type CloudflareErrorLogGroups = Vec<CloudflareErrorLogGroup>;
