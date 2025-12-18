#[cfg(feature = "vercel")]
use derive_getters::Getters;
#[cfg(feature = "vercel")]
use miette::{IntoDiagnostic, Result, miette};
#[cfg(feature = "vercel")]
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
#[cfg(feature = "vercel")]
use reqwest::Client;
#[cfg(feature = "vercel")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "vercel")]
use std::sync::Arc;
#[cfg(feature = "vercel")]
use tokio::sync::Semaphore;
#[cfg(feature = "vercel")]
use tracing::{debug, info};
#[cfg(feature = "vercel")]
use url::Url;

#[cfg(feature = "vercel")]
use crate::artifacts::VercelFileManifest;

#[cfg(feature = "vercel")]
#[derive(Clone, Getters)]
pub struct VercelClient {
    client: Client,
    /// The Vercel API token
    api_token: String,
    /// The Vercel team ID (optional)
    team_id: Option<String>,
    /// The project name
    project_name: String,
}

#[cfg(feature = "vercel")]
impl VercelClient {
    pub fn new(api_token: String, project_name: String, team_id: Option<String>) -> Self {
        let mut default_headers = HeaderMap::new();
        let auth = format!("Bearer {}", api_token);
        let mut auth_value = HeaderValue::from_str(&auth).expect("Must be able to set header");
        auth_value.set_sensitive(true);
        default_headers.insert(AUTHORIZATION, auth_value);

        let client = Client::builder()
            .default_headers(default_headers)
            .build()
            .expect("Must be able to construct client");

        Self {
            client,
            api_token,
            team_id,
            project_name,
        }
    }

    fn base_url() -> &'static str {
        "https://api.vercel.com"
    }

    /// Upload files to Vercel using parallel uploads with a tokio WaitGroup pattern
    /// Each file is uploaded individually with its SHA1 hash
    pub async fn upload_files(&self, manifest: &VercelFileManifest) -> Result<()> {
        info!("Uploading {} files to Vercel in parallel", manifest.files().len());

        // Create a semaphore to limit concurrent uploads (max 10 at a time)
        let semaphore = Arc::new(Semaphore::new(10));
        let mut upload_tasks = Vec::new();

        for file_entry in manifest.files() {
            let permit = semaphore.clone();
            let client = self.client.clone();
            let file_path = file_entry.path().to_path_buf();
            let sha1 = file_entry.sha1().to_string();
            let team_id = self.team_id.clone();

            let task = tokio::spawn(async move {
                let _permit = permit.acquire().await.unwrap();

                debug!("Uploading file: {:?} with SHA1: {}", file_path, sha1);

                // Read file contents
                let file_bytes = tokio::fs::read(&file_path).await.into_diagnostic()?;

                // Build the upload URL
                let mut url = Url::parse(&format!("{}/v2/now/files", Self::base_url()))
                    .into_diagnostic()?;

                if let Some(team_id) = team_id {
                    url.query_pairs_mut().append_pair("teamId", &team_id);
                }

                // Create headers
                let mut headers = HeaderMap::new();
                headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
                headers.insert("x-vercel-digest", HeaderValue::from_str(&sha1).into_diagnostic()?);
                headers.insert("Content-Length", HeaderValue::from_str(&file_bytes.len().to_string()).into_diagnostic()?);

                // Upload the file
                let response = client
                    .post(url)
                    .headers(headers)
                    .body(file_bytes)
                    .send()
                    .await
                    .into_diagnostic()?;

                if !response.status().is_success() {
                    let error_text = response.text().await.into_diagnostic()?;
                    return Err(miette!(
                        "Failed to upload file {:?}. Error: {}",
                        file_path,
                        error_text
                    ));
                }

                debug!("Successfully uploaded file: {:?}", file_path);
                Ok::<(), miette::Report>(())
            });

            upload_tasks.push(task);
        }

        // Wait for all uploads to complete (WaitGroup pattern)
        let results = futures_util::future::join_all(upload_tasks).await;

        // Check if any uploads failed
        for result in results {
            result.into_diagnostic()??;
        }

        info!("All files uploaded successfully");
        Ok(())
    }

    /// Create a deployment after files have been uploaded
    pub async fn create_deployment(
        &self,
        manifest: &VercelFileManifest,
        project_name: &str,
    ) -> Result<CreateDeploymentResponse> {
        debug!("Creating Vercel deployment");

        let mut url = Url::parse(&format!("{}/v13/deployments", Self::base_url()))
            .into_diagnostic()?;

        if let Some(team_id) = &self.team_id {
            url.query_pairs_mut().append_pair("teamId", team_id);
        }

        // Build the files array with SHA1 hashes
        let files: Vec<DeploymentFile> = manifest
            .files()
            .iter()
            .map(|entry| {
                let file_path = entry.path();
                let path_str = file_path
                    .to_str()
                    .unwrap_or("")
                    .to_string();

                DeploymentFile {
                    file: path_str,
                    sha: entry.sha1().to_string(),
                    size: 0, // Vercel doesn't strictly require size in the API
                }
            })
            .collect();

        let request = CreateDeploymentRequest {
            name: project_name.to_string(),
            files,
            project_settings: None,
            target: Some("production".to_string()),
        };

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            let error_text = response.text().await.into_diagnostic()?;
            return Err(miette!("Failed to create deployment. Error: {}", error_text));
        }

        let deployment_response = response
            .json::<CreateDeploymentResponse>()
            .await
            .into_diagnostic()?;

        debug!("Deployment created successfully");
        Ok(deployment_response)
    }

    /// Get the current deployment for a project
    pub async fn get_current_deployment(&self) -> Result<String> {
        debug!("Getting current Vercel deployment");

        let mut url = Url::parse(&format!(
            "{}/v9/projects/{}",
            Self::base_url(),
            self.project_name
        ))
        .into_diagnostic()?;

        if let Some(team_id) = &self.team_id {
            url.query_pairs_mut().append_pair("teamId", team_id);
        }

        let response = self.client.get(url).send().await.into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!("Failed to get current deployment"));
        }

        let project_response = response
            .json::<ProjectResponse>()
            .await
            .into_diagnostic()?;

        // Return the latest production deployment ID
        project_response
            .targets
            .and_then(|t| t.production)
            .and_then(|p| p.id)
            .ok_or_else(|| miette!("No production deployment found"))
    }
}

#[cfg(feature = "vercel")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentFile {
    pub file: String,
    pub sha: String,
    pub size: u64,
}

#[cfg(feature = "vercel")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDeploymentRequest {
    pub name: String,
    pub files: Vec<DeploymentFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_settings: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[cfg(feature = "vercel")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDeploymentResponse {
    pub id: String,
    pub url: String,
}

#[cfg(feature = "vercel")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectResponse {
    pub targets: Option<ProjectTargets>,
}

#[cfg(feature = "vercel")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectTargets {
    pub production: Option<DeploymentTarget>,
}

#[cfg(feature = "vercel")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentTarget {
    pub id: Option<String>,
}
