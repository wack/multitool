use miette::{IntoDiagnostic, Result};
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloudFlareError {
    pub code: i64,
    pub message: String,
    pub documentation_url: Option<String>,
    #[serde(default)]
    pub source: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloudFlareMessage {
    pub code: i64,
    pub message: String,
    pub documentation_url: Option<String>,
    #[serde(default)]
    pub source: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CloudFlareResponse<T> {
    pub errors: Vec<CloudFlareError>,
    pub messages: Vec<CloudFlareMessage>,
    pub success: bool,
    pub result: T,
}

static URL: OnceLock<Url> = OnceLock::new();

fn init_url() -> Url {
    Url::parse("https://api.cloudflare.com/client/v4/").unwrap()
}

#[derive(Clone)]
pub struct CloudFlareClient {
    client: Client,
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
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/create/
    pub fn create_deployment(&self) -> Result<()> {
        todo!();
    }

    // For the monitor to grab metrics within a time range.
    pub fn collect_metrics(&self) -> Result<()> {
        todo!();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cloudflare_response_serde() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct TestResult {
            startup_time_ms: i64,
        }

        let json_str = r#"
{
    "errors": [
        {
            "code": 1000,
            "message": "message",
            "documentation_url": "documentation_url",
            "source": {
                "pointer": "pointer"
            }
        }
    ],
    "messages": [
        {
            "code": 1000,
            "message": "message",
            "documentation_url": "documentation_url",
            "source": {
                "pointer": "pointer"
            }
        }
    ],
    "success": true,
    "result": {
        "startup_time_ms": 10
    }
}"#;

        // Test deserialization
        let response: CloudFlareResponse<TestResult> = serde_json::from_str(json_str).unwrap();
        
        assert_eq!(response.success, true);
        assert_eq!(response.errors.len(), 1);
        assert_eq!(response.errors[0].code, 1000);
        assert_eq!(response.errors[0].message, "message");
        assert_eq!(response.errors[0].documentation_url, Some("documentation_url".to_string()));
        
        assert_eq!(response.messages.len(), 1);
        assert_eq!(response.messages[0].code, 1000);
        assert_eq!(response.messages[0].message, "message");
        assert_eq!(response.messages[0].documentation_url, Some("documentation_url".to_string()));
        
        assert_eq!(response.result.startup_time_ms, 10);

        // Test serialization
        let serialized = serde_json::to_string_pretty(&response).unwrap();
        let deserialized: CloudFlareResponse<TestResult> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(response, deserialized);
    }
}
