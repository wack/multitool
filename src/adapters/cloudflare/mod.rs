use miette::{IntoDiagnostic, Result};
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tracing::error;
use url::Url;

static URL: OnceLock<Url> = OnceLock::new();

fn init_url() -> Url {
    Url::parse("https://api.cloudflare.com/client/v4/").unwrap()
}

#[derive(Clone)]
pub struct CloudFlareClient {
    client: Client,
}

#[derive(Deserialize)]
struct CloudFlareDeploymentResponse {
    result: CloudFlareDeploymentResult,
}

#[derive(Deserialize)]
struct CloudFlareDeploymentResult {
    deployments: Vec<CloudFlareDeployment>,
}

#[derive(Deserialize)]
struct CloudFlareDeployment {
    id: String,
}

impl CloudFlareClient {
    pub fn new(token: &str) -> Self {
        // TODO: Add a timeout.
        let mut default_headers = HeaderMap::new();
        let auth = format!("Bearer {token}");
        let mut auth_value = HeaderValue::from_str(&auth).expect("Must be able to set header");
        auth_value.set_sensitive(true);
        default_headers.insert(AUTHORIZATION, auth_value);
        let client = Client::builder()
            .default_headers(default_headers)
            .build()
            .expect("Must be able to construct client");
        Self { client }
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/versions/methods/create/
    pub async fn upload_version(
        &self,
        account_id: String,
        script_name: String,
        metadata: Metadata,
    ) -> Result<()> {
        let path =
            format!("/accounts/{account_id}/workers/scripts/{script_name}/assets-upload-session");
        let url = Self::url_with_path(&path);
        self.client.post(url).send().await.into_diagnostic()?;
        Ok(())
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/assets/subresources/upload/methods/create/
    pub fn create_assets_upload_session(&self) -> Result<()> {
        todo!();
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/assets/subresources/upload/methods/create/
    pub fn upload_assets(&self) -> Result<()> {
        todo!();
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/get/
    pub async fn get_current_version(
        &self,
        account_id: String,
        script_name: String,
    ) -> Result<String> {
        let path = format!("/accounts/{account_id}/workers/scripts/{script_name}/deployments");
        let url = Self::url_with_path(&path);

        let response = self.client.get(url).send().await.into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette::miette!(
                "Failed to get current worker version. Error: {:?}",
                response
                    .json()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string())
            ));
        }

        let deployment_response = response
            .json::<CloudFlareDeploymentResponse>()
            .await
            .into_diagnostic()?;

        // The cloudflare API auto-sorts deployments so the first deployment listed is the currently active deployment
        deployment_response
            .result
            .deployments
            .first()
            .map(|deployment| deployment.id.clone())
            .ok_or_else(|| miette::miette!("No deployments found"))
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/create/
    pub fn create_deployment(&self) -> Result<()> {
        todo!();
    }

    // For the monitor to grab metrics within a time range.
    pub async fn collect_metrics(
        &self,
        account_id: String,
        worker_name: String,
        worker_version_id: String,
        status_code_range_start: u16,
        status_code_range_end: u16,
        from_time: chrono::DateTime<chrono::Utc>,
        to_time: chrono::DateTime<chrono::Utc>,
    ) -> Result<u32> {
        let path = format!("/accounts/{account_id}/workers/observability/telemetry/query");
        let url = Self::url_with_path(&path);

        // Convert DateTime to Unix timestamps
        let from_timestamp = from_time.timestamp() as u64;
        let to_timestamp = to_time.timestamp() as u64;

        let query_body = serde_json::json!({
            "view": "calculations",
            "queryId": "worker-calculation",
            "parameters": {
                "datasets": ["cloudflare-workers"],
                "filters": [
                    {
                        "key": "$metadata.service",
                        "operation": "eq",
                        "type": "string",
                        "value": worker_name
                    },
                    {
                        "key": "$workers.scriptVersion.id",
                        "operation": "eq",
                        "type": "string",
                        "value": worker_version_id
                    },
                    {
                        "key": "$workers.event.response.status",
                        "operation": "gte",
                        "type": "number",
                        "value": status_code_range_start
                    },
                    {
                        "key": "$workers.event.response.status",
                        "operation": "lte",
                        "type": "number",
                        "value": status_code_range_end
                    }
                ],
                "calculations": [
                    {
                        "key": "$workers.event.response.status",
                        "operator": "count",
                        "keyType": "number",
                        "alias": "sum"
                    }
                ]
            },
            "timeframe": {
                "to": to_timestamp,
                "from": from_timestamp
            }
        });

        let response = self
            .client
            .post(url)
            .json(&query_body)
            .send()
            .await
            .into_diagnostic()?;

        // If there's an error, just return 0 results
        if !response.status().is_success() {
            error!(
                "Failed to query Cloudflare metrics for worker: {}, version: {}, status codes: {}, error: {}",
                worker_name,
                worker_version_id,
                status_code_range_start,
                response
                    .json()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string())
            );
            return Ok(0);
        }

        let metrics_response = response
            .json::<MetricsApiResponse>()
            .await
            .into_diagnostic()?;

        let count = metrics_response
            .result
            .and_then(|r| r.calculations.get(0).cloned())
            .and_then(|c| c.aggregates.get(0).cloned())
            .map_or(0, |a| a.count);

        Ok(count)
    }

    fn base_url() -> &'static Url {
        URL.get_or_init(init_url)
    }

    fn url_with_path(path: &str) -> Url {
        let mut url = Self::base_url().clone();
        url.set_path(path);
        url
    }
}

/// Ugh, I started implementing this elsewhere but i dont have the code on my laptop right now.
#[derive(Serialize, Deserialize)]
pub struct Metadata;

#[derive(Deserialize)]
struct MetricsApiResponse {
    result: Option<MetricsResult>,
}

#[derive(Deserialize)]
struct MetricsResult {
    #[serde(default)]
    calculations: Vec<Calculation>,
}

#[derive(Deserialize, Clone)]
struct Calculation {
    #[serde(default)]
    aggregates: Vec<Aggregate>,
}

#[derive(Deserialize, Clone)]
struct Aggregate {
    #[serde(default)]
    count: u32,
}
