use chrono::DateTime;
use derive_getters::Getters;
use miette::{IntoDiagnostic, Result, miette};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::multipart::Part;
use reqwest::{Client, multipart};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tokio::fs::read;
use tracing::{debug, error};
use uploads::{UploadVersionRequest, UploadVersionResponse};
use url::Url;

use deployments::{CreateDeploymentRequest, DeploymentResponse};
use metrics::MetricsResponse;
use responses::CloudflareResponse;

use crate::artifacts::CloudflareManifest;

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
pub struct WorkerScript {
    id: String,
    created_on: String,
    modified_on: String,
    usage_model: Option<String>,
    etag: String,
}

static URL: OnceLock<Url> = OnceLock::new();

fn init_url() -> Url {
    // NOTE: The trailing slash is important here, otherwise the URL parsing will fail!
    Url::parse("https://api.cloudflare.com/client/v4/").unwrap()
}

#[derive(Clone, Getters)]
pub struct CloudflareClient {
    client: Client,
    /// This is the Cloudflare account id
    account_id: String,
}

impl CloudflareClient {
    pub fn new(account_id: String, token: &str) -> Self {
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

        Self { client, account_id }
    }

    // Commented out until we verify if we need an upload session.
    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/assets/subresources/upload/methods/create/
    // pub async fn create_assets_upload_session(
    //     &self,
    //     manifest: &CloudflareManifest,
    // ) -> Result<UploadSessionResponse> {
    //     debug!("Creating assets upload session");
    //     let account_id = &self.account_id;
    //     let worker_name = &self.worker_name;
    //     let path =
    //         format!("accounts/{account_id}/workers/scripts/{worker_name}/assets-upload-session");
    //     let url = Self::url_with_path(&path);

    //     let response = self
    //         .client
    //         .post(url)
    //         .json(manifest)
    //         .send()
    //         .await
    //         .into_diagnostic()?;

    //     if !response.status().is_success() {
    //         return Err(miette!(
    //             "Failed to create assets upload session. Error: {:?}",
    //             response.json::<serde_json::Value>().await
    //         ));
    //     }

    //     let upload_session_response = response
    //         .json::<CloudflareResponse<UploadSessionResponse>>()
    //         .await
    //         .into_diagnostic()?;

    //     debug!("Assets upload session created successfully");
    //     Ok(upload_session_response.result)
    // }

    // Commented out until we verify if we need an upload session.
    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/assets/subresources/upload/methods/create/
    // pub async fn upload_assets(
    //     &self,
    //     file_upload_jwt: &str,
    //     bucket: Vec<String>,
    //     manifest: CloudflareManifest,
    // ) -> Result<UploadAssetsResponse> {
    //     debug!("Uploading assets");
    //     let account_id = &self.account_id;
    //     let path = format!("accounts/{account_id}/workers/assets/upload?base64=true");
    //     let url = Self::url_with_path(&path);

    //     let mut request = multipart::Form::new();

    //     let mut files = manifest.files().clone();
    //     files.sort_by_key(|file| file.digest());
    //     // Loops over the file hashes from the bucket CF returns
    //     // Looks up the file in the manifest and adds the base64 content to the request
    //     for file_hash in bucket {
    //         let file_idx = files
    //             .binary_search_by_key(&file_hash, |file| file.digest())
    //             .map_err(|_| miette!("File hash {file_hash} not found in manifest"))?;
    //         let path = files[file_idx].path().to_path_buf();
    //         files.remove(file_idx);

    //         let mut file_bytes = Vec::new();
    //         read_file_as_b64(path, &mut file_bytes).await?;

    //         let file_len = file_bytes.len() as u64;

    //         request = request.part(file_hash, Part::stream_with_length(file_bytes, file_len));
    //     }

    //     let mut file_upload_headers = HeaderMap::new();

    //     // Set the authorization header with the JWT token we got from the session upload request
    //     let file_upload_auth = format!("Bearer {file_upload_jwt}");
    //     let mut file_upload_jwt_header = HeaderValue::from_str(&file_upload_auth).unwrap();
    //     file_upload_jwt_header.set_sensitive(true);
    //     file_upload_headers.insert(AUTHORIZATION, file_upload_jwt_header);

    //     let response = self
    //         .client
    //         .post(url.clone())
    //         // TODO: during testing, check if this overrides the default headers
    //         .headers(file_upload_headers)
    //         .multipart(request)
    //         .send()
    //         .await
    //         .into_diagnostic()?;

    //     if !response.status().is_success() {
    //         return Err(miette!(
    //             "Failed to upload asset(s). Error: {:?}",
    //             response.json::<serde_json::Value>().await
    //         ));
    //     }

    //     let upload_response = response
    //         .json::<CloudflareResponse<UploadAssetsResponse>>()
    //         .await
    //         .into_diagnostic()?;

    //     debug!("Assets uploaded successfully");
    //     Ok(upload_response.result)
    // }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/versions/methods/create/
    pub async fn upload_version(
        &self,
        worker_name: &String,
        manifest: &CloudflareManifest,
        main_module: &String,
    ) -> Result<UploadVersionResponse> {
        debug!("Uploading Worker version");
        let account_id = &self.account_id;
        let path = format!("accounts/{account_id}/workers/scripts/{worker_name}/versions");
        let url = Self::url_with_path(&path);

        let metadata = UploadVersionRequest::new(main_module.to_owned());

        let mut request = multipart::Form::new().text(
            "metadata",
            serde_json::to_string(&metadata).into_diagnostic()?,
        );

        for file_path in manifest.files() {
            let file_bytes = read(file_path).await.into_diagnostic()?;
            let mut file_part = Part::bytes(file_bytes);

            // Ensure we keep the whole file path (with the root stripped) as the name, not just the individual file name
            let file_path_str = file_path
                .strip_prefix(&manifest.root())
                .expect("Must be able to strip prefix")
                .to_str()
                .unwrap()
                .to_string();
            file_part = file_part.file_name(file_path_str.clone());
            file_part = file_part.mime_str("application/javascript+module").unwrap();
            request = request.part(file_path_str, file_part);
        }

        let response = self
            .client
            .post(url)
            .multipart(request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to upload Worker version. Error: {:?}",
                response
                    .json::<CloudflareResponse<serde_json::Value>>()
                    .await
            ));
        }

        let version_response = response
            .json::<CloudflareResponse<UploadVersionResponse>>()
            .await
            .into_diagnostic()?;

        debug!("Worker version uploaded successfully");
        Ok(version_response.result)
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/get/
    pub async fn get_current_version(&self, worker_name: &String) -> Result<String> {
        debug!("Getting current Worker version");
        let account_id = &self.account_id;
        let path = format!("accounts/{account_id}/workers/scripts/{worker_name}/deployments");
        let url = Self::url_with_path(&path);

        let response = self.client.get(url).send().await.into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to get current Worker version. Error: {:?}",
                response.json::<serde_json::Value>().await
            ));
        }

        let deployment_response = response
            .json::<CloudflareResponse<DeploymentResponse>>()
            .await
            .into_diagnostic()?;

        debug!("Current Worker version retrieved successfully");
        // The cloudflare API auto-sorts deployments so the first deployment listed is the currently active deployment
        let version_id = deployment_response
            .result
            .deployments
            .first()
            .ok_or(miette!("No deployments found"))?
            .versions
            .first()
            .ok_or(miette!("No deployment versions found"))?
            .version_id
            .clone();

        Ok(version_id)
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/create/
    pub async fn create_deployment(
        &self,
        worker_name: &String,
        request: CreateDeploymentRequest,
    ) -> Result<()> {
        debug!("Deploying updated version(s)");
        let account_id = &self.account_id;
        let path = format!("accounts/{account_id}/workers/scripts/{worker_name}/deployments");
        let url = Self::url_with_path(&path);

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to create new Worker deployment. Error: {:?}",
                response.json::<serde_json::Value>().await
            ));
        }
        debug!("Worker deployment successful");
        Ok(())
    }

    // For the monitor to grab metrics within a time range.
    pub async fn collect_metrics(
        &self,
        worker_name: &String,
        worker_version_id: &String,
        status_code_range_start: u16,
        status_code_range_end: u16,
        from_time: DateTime<chrono::Utc>,
        to_time: DateTime<chrono::Utc>,
    ) -> Result<u32> {
        let account_id = &self.account_id;
        let path = format!("accounts/{account_id}/workers/observability/telemetry/query");
        let url = Self::url_with_path(&path);

        // Convert DateTime to Unix Millisecond timestamps
        let from_timestamp = from_time.timestamp_millis() as u64;
        let to_timestamp = to_time.timestamp_millis() as u64;

        let query_body = serde_json::json!({
          "view": "calculations",
          "queryId": "worker-calculation",
          "parameters": {
            "datasets": [
              "cloudflare-workers"
            ],
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
                "Failed to query Cloudflare metrics for worker: {}, version: {}, status codes: {}-{}, error: {:?}",
                worker_name,
                worker_version_id,
                status_code_range_start,
                status_code_range_end,
                response.json::<serde_json::Value>().await
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

    // List all workers for the account
    // Corresponds to: https://developers.cloudflare.com/api/resources/workers/subresources/scripts/methods/list/
    pub async fn list_workers(&self) -> Result<Vec<WorkerScript>> {
        debug!("Listing Cloudflare Workers");
        let account_id = &self.account_id;
        let path = format!("accounts/{account_id}/workers/scripts");
        let url = Self::url_with_path(&path);

        // Make request to Cloudflare API
        let response = self.client.get(url).send().await.into_diagnostic()?;

        // Check if we get a non-2xx response
        if !response.status().is_success() {
            return Err(miette!(
                "Failed to list Workers. Error: {:?}",
                response.json::<serde_json::Value>().await
            ));
        }

        // Serialize the response into our struct
        let workers_response = response
            .json::<CloudflareResponse<Vec<WorkerScript>>>()
            .await
            .into_diagnostic()?;

        debug!("Workers listed successfully");
        Ok(workers_response.result)
    }

    fn base_url() -> &'static Url {
        URL.get_or_init(init_url)
    }

    /// Adds the URL path to the base URL.
    /// All paths MUST NOT start with a slash.
    fn url_with_path(path: &str) -> Url {
        let url = Self::base_url().clone();
        url.join(path).unwrap()
    }
}

pub mod deployments;
mod metrics;
mod responses;
mod uploads;
