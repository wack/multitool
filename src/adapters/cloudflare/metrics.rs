use std::collections::HashMap;

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
    pub invocations: HashMap<String, Vec<InvocationData>>,
}

#[derive(Deserialize, Debug)]
pub struct InvocationData {
    #[serde(rename = "$workers")]
    pub workers: WorkersData,
    #[serde(rename = "$metadata")]
    pub metadata: MetadataData,
    pub source: SourceData,
}

#[derive(Deserialize, Debug)]
pub struct WorkersData {
    pub event: EventData,
}

#[derive(Deserialize, Debug)]
pub struct EventData {
    pub request: RequestData,
    #[serde(default)]
    pub response: Option<ResponseData>,
}

#[derive(Deserialize, Debug)]
pub struct RequestData {
    pub url: String,
    pub method: String,
    pub path: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ResponseData {
    pub status: i32,
}

#[derive(Deserialize, Debug)]
pub struct MetadataData {
    #[serde(rename = "type")]
    pub event_type: String,
}

#[derive(Deserialize, Debug)]
pub struct SourceData {
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct CloudflareErrorLog {
    pub method: String,
    pub path: String,
    pub status_code: i32,
    pub logs: Vec<String>,
}

impl Into<Vec<CloudflareErrorLog>> for ErrorLogsResponse {
    fn into(self) -> Vec<CloudflareErrorLog> {
        let mut error_logs = Vec::new();

        for (_request_id, invocations) in self.invocations {
            // The cf-worker-event entry contains the request/response details
            let event_entry = invocations
                .iter()
                .find(|inv| inv.metadata.event_type == "cf-worker-event");

            if let Some(event) = event_entry {
                let request = &event.workers.event.request;
                let method = request.method.clone();
                let path = request.path.clone();

                // Extract log line from each invocation event and combine
                if let Some(response) = &event.workers.event.response {
                    let mut logs: Vec<String> = invocations
                        .iter()
                        .map(|inv| inv.source.message.clone())
                        .collect();

                    logs.reverse(); // Reverse to maintain chronological order

                    let error_log = CloudflareErrorLog {
                        method,
                        path,
                        status_code: response.status,
                        logs,
                    };

                    error_logs.push(error_log);
                }
            }
        }

        error_logs
    }
}
