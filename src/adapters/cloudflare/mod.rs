use chrono::DateTime;
use miette::{IntoDiagnostic, Result};
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use std::collections::HashMap;
use std::sync::OnceLock;
use tracing::error;
use uploads::{UploadAssetsResponse, UploadRequest, UploadSessionResponse, UploadVersionRequest};
use url::Url;

use deployments::{CreateDeploymentRequest, DeploymentResponse};
use metrics::MetricsResponse;
use responses::CloudflareResponse;

static URL: OnceLock<Url> = OnceLock::new();

fn init_url() -> Url {
    Url::parse("https://api.cloudflare.com/client/v4/").unwrap()
}

#[derive(Clone)]
pub struct CloudflareClient {
    client: Client,
}

impl CloudflareClient {
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
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/assets/subresources/upload/methods/create/
    pub async fn create_assets_upload_session(
        &self,
        account_id: String,
        worker_name: String,
        request: UploadRequest,
    ) -> Result<UploadSessionResponse> {
        let path =
            format!("/accounts/{account_id}/workers/scripts/{worker_name}/assets-upload-session");
        let url = Self::url_with_path(&path);

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette::miette!(
                "Failed to create assets upload session. Error: {:?}",
                response
                    .json()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string())
            ));
        }

        let upload_session_response = response
            .json::<CloudflareResponse<UploadSessionResponse>>()
            .await
            .into_diagnostic()?;

        Ok(upload_session_response.result)
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/assets/subresources/upload/methods/create/
    pub async fn upload_assets(
        &self,
        account_id: String,
        file_upload_jwt: String,
        // Key: file hash, Value: base64 encoded file
        files: HashMap<String, String>,
        // The list of file hashes Cloudflare wants us to upload together
        bucket: Vec<String>,
    ) -> Result<String> {
        let path = format!("/accounts/{account_id}/workers/assets/upload?base64=true");
        let url = Self::url_with_path(&path);

        let mut request = HashMap::new();

        // Loops over the file hashes and adds them to a multipart/form-data request
        for file_hash in bucket {
            let base64_data = files
                .get(&file_hash)
                .ok_or_else(|| miette::miette!("File not found in the list"))?;

            request.insert(file_hash.clone(), base64_data);
        }

        let mut file_upload_headers = HeaderMap::new();

        // Set the content type to multipart/form-data
        file_upload_headers.insert(CONTENT_TYPE, "multipart/form-data".parse().unwrap());

        // Set the authorization header with the JWT token we got from the session upload request
        let file_upload_auth = format!("Bearer {file_upload_jwt}");
        let mut file_upload_jwt_header = HeaderValue::from_str(&file_upload_auth).unwrap();
        file_upload_jwt_header.set_sensitive(true);
        file_upload_headers.insert(AUTHORIZATION, file_upload_jwt_header);

        let response = self
            .client
            .post(url.clone())
            // TODO: during testing, check if this overrides the default headers
            .headers(file_upload_headers)
            .json(&request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette::miette!(
                "Failed to upload asset(s). Error: {:?}",
                response
                    .json()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string())
            ));
        }

        let upload_response = response
            .json::<CloudflareResponse<UploadAssetsResponse>>()
            .await
            .into_diagnostic()?;

        Ok(upload_response.result.jwt)
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/versions/methods/create/
    pub async fn upload_version(
        &self,
        account_id: String,
        worker_name: String,
        file_upload_jwt: String,
    ) -> Result<()> {
        let path = format!("/accounts/{account_id}/workers/scripts/{worker_name}");
        let url = Self::url_with_path(&path);

        let request = UploadVersionRequest::new(file_upload_jwt);

        let response = self
            .client
            .put(url)
            .json(&request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette::miette!(
                "Failed to upload Worker version. Error: {:?}",
                response
                    .json()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string())
            ));
        }

        Ok(())
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
                "Failed to get current Worker version. Error: {:?}",
                response
                    .json()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string())
            ));
        }

        let deployment_response = response
            .json::<CloudflareResponse<DeploymentResponse>>()
            .await
            .into_diagnostic()?;

        // The cloudflare API auto-sorts deployments so the first deployment listed is the currently active deployment
        deployment_response
            .result
            .deployments
            .first()
            .map(|deployment| deployment.id().clone())
            .ok_or_else(|| miette::miette!("No deployments found"))
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/create/
    pub async fn create_deployment(
        &self,
        account_id: String,
        script_name: String,
        request: CreateDeploymentRequest,
    ) -> Result<()> {
        let path = format!("accounts/{account_id}/workers/scripts/{script_name}/deployments");
        let url = Self::url_with_path(&path);

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette::miette!(
                "Failed to create new Worker deployment. Error: {:?}",
                response
                    .json()
                    .await
                    .unwrap_or_else(|_| "Unknown error".to_string())
            ));
        }

        Ok(())
    }

    // For the monitor to grab metrics within a time range.
    pub async fn collect_metrics(
        &self,
        account_id: String,
        worker_name: String,
        worker_version_id: String,
        status_code_range_start: u16,
        status_code_range_end: u16,
        from_time: DateTime<chrono::Utc>,
        to_time: DateTime<chrono::Utc>,
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
            .json::<CloudflareResponse<MetricsResponse>>()
            .await
            .into_diagnostic()?;

        let count = metrics_response
            .result
            .calculations
            .get(0)
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

pub mod deployments;
mod metrics;
mod responses;
mod uploads;
