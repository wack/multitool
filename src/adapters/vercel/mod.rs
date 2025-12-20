use std::sync::OnceLock;

use derive_getters::Getters;
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use url::Url;

static URL: OnceLock<Url> = OnceLock::new();

fn init_url() -> Url {
    // NOTE: The trailing slash is important here, otherwise the URL parsing will fail!
    Url::parse("https://api.vercel.com/").unwrap()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TeamIdentifier {
    Id { team_id: String },
    Slug { slug: String },
}

impl TeamIdentifier {
    pub fn id(id: impl Into<String>) -> Self {
        Self::Id { team_id: id.into() }
    }

    pub fn slug(slug: impl Into<String>) -> Self {
        Self::Slug { slug: slug.into() }
    }
}

#[derive(Clone, Getters)]
pub struct VercelClient {
    http: Client,
    url: Url,
    team: Option<TeamIdentifier>,
}

impl VercelClient {
    pub fn new(token: impl AsRef<str>, team: Option<TeamIdentifier>) -> anyhow::Result<Self> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", token.as_ref()))?,
        );
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        let http = Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self {
            http,
            url: URL.get_or_init(init_url).clone(),
            team,
        })
    }

    fn add_team_query(&self, url: &mut Url) {
        if let Some(team) = &self.team {
            match team {
                TeamIdentifier::Id { team_id } => {
                    url.query_pairs_mut().append_pair("teamId", team_id);
                }
                TeamIdentifier::Slug { slug } => {
                    url.query_pairs_mut().append_pair("slug", slug);
                }
            }
        }
    }

    pub async fn get_project(&self, id_or_name: impl AsRef<str>) -> anyhow::Result<Project> {
        let mut url = self.url.clone();
        url.set_path(&format!("v9/projects/{}", id_or_name.as_ref()));
        self.add_team_query(&mut url);

        let response = self.http.get(url).send().await?;
        
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            anyhow::bail!("Failed to get project: {} - {}", status, text);
        }

        let project = response.json::<Project>().await?;
        Ok(project)
    }
}

// Response types for the project endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub account_id: String,
    pub updated_at: Option<u64>,
    pub created_at: u64,
    pub framework: Option<String>,
    pub dev_command: Option<String>,
    pub build_command: Option<String>,
    pub output_directory: Option<String>,
    pub root_directory: Option<String>,
    pub directory_listing: bool,
    pub node_version: String,
    pub live: Option<bool>,
    pub env: Option<Vec<EnvVariable>>,
    pub git_fork_protection: Option<bool>,
    pub git_lfs: Option<bool>,
    pub link: Option<serde_json::Value>,
    pub source_files_outside_root_directory: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvVariable {
    pub key: String,
    pub value: String,
    #[serde(rename = "type")]
    pub var_type: String,
    pub id: Option<String>,
    pub created_at: Option<u64>,
    pub updated_at: Option<u64>,
    pub target: Option<Vec<String>>,
}
