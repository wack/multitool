use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::CloudFlareResponse;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DeploymentStrategy {
    #[serde(rename = "percentage")]
    Percentage,
}

impl Default for DeploymentStrategy {
    fn default() -> Self {
        Self::Percentage
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateDeploymentRequest {
    #[serde(default)]
    pub strategy: DeploymentStrategy,
    pub versions: Vec<DeploymentVersionConfig>,
    pub message: Option<String>,
    pub annotations: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct DeploymentResult {
    pub id: String,
    pub metadata: Value,
    pub created_on: String,
    pub modified_on: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeploymentVersionConfig {
    pub percentage: u32,
    pub version_id: String,
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_deployment_request_serde() {
        // Test case 1: Basic deployment request
        let json_str = r#"{
            "strategy": "percentage",
            "versions": [
                {
                    "percentage": 100,
                    "version_id": "bcf48806-b317-4351-9ee7-36e7d557d4de"
                }
            ]
        }"#;

        let request: CreateDeploymentRequest = serde_json::from_str(json_str).unwrap();

        assert_eq!(request.strategy, DeploymentStrategy::Percentage);
        assert_eq!(request.versions.len(), 1);
        assert_eq!(request.versions[0].percentage, 100);
        assert_eq!(
            request.versions[0].version_id,
            "bcf48806-b317-4351-9ee7-36e7d557d4de"
        );
        assert_eq!(request.message, None);
        assert_eq!(request.annotations, None);

        // Test case 2: Request with message and annotations
        let json_str = r#"{
            "strategy": "percentage",
            "versions": [
                {
                    "percentage": 80,
                    "version_id": "abc123"
                },
                {
                    "percentage": 20,
                    "version_id": "def456"
                }
            ],
            "message": "Deploying new version",
            "annotations": {
                "key": "value"
            }
        }"#;

        let request: CreateDeploymentRequest = serde_json::from_str(json_str).unwrap();

        assert_eq!(request.strategy, DeploymentStrategy::Percentage);
        assert_eq!(request.versions.len(), 2);
        assert_eq!(request.versions[0].percentage, 80);
        assert_eq!(request.versions[0].version_id, "abc123");
        assert_eq!(request.versions[1].percentage, 20);
        assert_eq!(request.versions[1].version_id, "def456");
        assert_eq!(request.message, Some("Deploying new version".to_string()));
        assert!(request.annotations.is_some());

        // Test case 3: Strategy should default to "percentage" when not provided
        let json_str = r#"{
            "versions": [
                {
                    "percentage": 100,
                    "version_id": "abc123"
                }
            ]
        }"#;

        let request: CreateDeploymentRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(request.strategy, DeploymentStrategy::Percentage);

        // Test serialization roundtrip
        let serialized = serde_json::to_string_pretty(&request).unwrap();
        let deserialized: CreateDeploymentRequest = serde_json::from_str(&serialized).unwrap();
        assert_eq!(request, deserialized);
    }

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
        assert_eq!(
            response.errors[0].documentation_url,
            Some("documentation_url".to_string())
        );

        assert_eq!(response.messages.len(), 1);
        assert_eq!(response.messages[0].code, 1000);
        assert_eq!(response.messages[0].message, "message");
        assert_eq!(
            response.messages[0].documentation_url,
            Some("documentation_url".to_string())
        );

        assert_eq!(response.result.startup_time_ms, 10);

        // Test serialization
        let serialized = serde_json::to_string_pretty(&response).unwrap();
        let deserialized: CloudFlareResponse<TestResult> =
            serde_json::from_str(&serialized).unwrap();
        assert_eq!(response, deserialized);
    }
}
