use bon::Builder;
use derive_getters::Getters;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum DeploymentStrategy {
    #[serde(rename = "percentage")]
    #[default]
    Percentage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Builder, Getters)]
pub struct CreateDeploymentRequest {
    #[serde(default)]
    pub strategy: DeploymentStrategy,
    pub versions: Vec<DeploymentVersion>,
    pub annotations: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Builder, Getters)]
pub struct DeploymentVersion {
    pub percentage: u64,
    pub version_id: String,
}

#[derive(Deserialize)]
pub struct DeploymentResponse {
    pub deployments: Vec<Deployment>,
}

#[derive(Deserialize)]
pub struct Deployment {
    pub versions: Vec<DeploymentVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Binding {
    Ai {
        name: String,
    },
    AnalyticsEngine {
        dataset: String,
        name: String,
    },
    Assets {
        name: String,
    },
    Browser {
        name: String,
    },
    D1 {
        id: String,
        name: String,
    },
    DispatchNamespace {
        name: String,
        namespace: String,
        outbound: Option<(Vec<String>, (String, String))>,
    },
    DurableObjectNamespace {
        name: String,
        class_name: Option<String>,
        environment: Option<String>,
        script_name: Option<String>,
    },
    Hyperdrive {
        id: String,
        name: String,
    },
    Json {
        json: String,
        name: String,
    },
    KvNamespace {
        name: String,
        namespace_id: String,
    },
    MtlsCertificate {
        certificate_id: String,
        name: String,
    },
    // PlainText is used for Environment variables in the Wrangler file
    PlainText {
        name: String,
        text: String,
    },
    Pipelines {
        name: String,
        pipeline: String,
    },
    Queue {
        name: String,
        queue_name: String,
    },
    R2Bucket {
        bucket_name: String,
        name: String,
    },
    SecretText {
        name: String,
        text: String,
    },
    Service {
        environment: String,
        name: String,
        service: String,
    },
    TailConsumer {
        name: String,
        service: String,
    },
    Vectorize {
        index_name: String,
        name: String,
    },
    VersionMetadata {
        name: String,
    },
    SecretsStoreSecret {
        name: String,
        secret_name: String,
        store_id: String,
    },
    SecretKey {
        algorithm: String,
        format: String,
        name: String,
        usages: Vec<String>,
        key_base64: Option<String>,
        key_jwk: Option<String>,
    },
    Workflow {
        name: String,
        workflow_name: String,
        class_name: Option<String>,
        script_name: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use crate::adapters::cloudflare::responses::CloudflareResponse;

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
        let response: CloudflareResponse<TestResult> = serde_json::from_str(json_str).unwrap();

        assert!(response.success);

        // Test errors
        let errors = response.errors.as_ref().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, 1000);
        assert_eq!(errors[0].message, "message");
        assert_eq!(
            errors[0].documentation_url,
            Some("documentation_url".to_string())
        );

        // Test messages
        let messages = response.messages.as_ref().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].code, Some(1000));
        assert_eq!(messages[0].message, Some("message".to_string()));
        assert_eq!(
            messages[0].documentation_url,
            Some("documentation_url".to_string())
        );

        assert_eq!(response.result.startup_time_ms, 10);
    }
}
