use bon::Builder;
use derive_getters::Getters;
use serde::{Deserialize, Serialize};

use crate::{adapters::cloudflare::deployments::Binding, fs::wrangler::Wrangler};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadSessionResponse {
    pub jwt: String,
    // Contains a list of lists of file hashes that Cloudflare wants us to upload together
    pub buckets: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadAssetsResponse {
    pub jwt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Getters, Builder)]
pub struct UploadVersionRequest {
    main_module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility_flags: Option<Vec<String>>,
    bindings: Option<Vec<Binding>>,
}

impl From<Wrangler> for UploadVersionRequest {
    fn from(wrangler: Wrangler) -> Self {
        let mut bindings = Vec::new();

        // The Wrangler file and the UploadVersionRequest share the same data
        // but in a different format, so we need to convert between the two.

        if wrangler.durable_objects().is_some() {
            for durable_object in wrangler.durable_objects().as_ref().unwrap() {
                bindings.push(Binding::DurableObjectNamespace {
                    name: durable_object.name.clone(),
                    class_name: Some(durable_object.class_name.clone()),
                    script_name: durable_object.script_name.clone(),
                    environment: durable_object.environment.clone(),
                });
            }
        }

        if wrangler.kv_namespaces().is_some() {
            for kv_namespace in wrangler.kv_namespaces().as_ref().unwrap() {
                bindings.push(Binding::KvNamespace {
                    name: kv_namespace.binding.clone(),
                    namespace_id: kv_namespace.id.clone(),
                });
            }
        }

        if wrangler.r2_buckets().is_some() {
            for r2_bucket in wrangler.r2_buckets().as_ref().unwrap() {
                bindings.push(Binding::R2Bucket {
                    name: r2_bucket.binding.clone(),
                    bucket_name: r2_bucket.bucket_name.clone(),
                });
            }
        }

        if wrangler.vectorize().is_some() {
            for vectorize in wrangler.vectorize().as_ref().unwrap() {
                bindings.push(Binding::Vectorize {
                    name: vectorize.binding.clone(),
                    index_name: vectorize.index_name.clone(),
                });
            }
        }

        if wrangler.services().is_some() {
            for service in wrangler.services().as_ref().unwrap() {
                bindings.push(Binding::Service {
                    name: service.binding.clone(),
                    service: service.service.clone(),
                    environment: service.environment.clone().unwrap_or_default(),
                });
            }
        }

        if wrangler.tail_consumers().is_some() {
            for tail_consumer in wrangler.tail_consumers().as_ref().unwrap() {
                bindings.push(Binding::TailConsumer {
                    name: tail_consumer.service.clone(),
                    service: tail_consumer.service.clone(),
                });
            }
        }

        return Self {
            main_module: wrangler.main().to_string(),
            compatibility_date: wrangler.compatibility_date().clone(),
            compatibility_flags: wrangler.compatibility_flags().clone(),
            bindings: Some(bindings),
        };
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadVersionResponse {
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upload_session_response_deserialization() {
        let json_str = r#"{
            "jwt": "upload-token-123",
            "buckets": [
                ["08f1dfda4574284ab3c21666d1", "4f1c1af44620d531446ceef93f"],
                ["54995e302614e0523757a04ec1"]
            ]
        }"#;

        let response: UploadSessionResponse = serde_json::from_str(json_str).unwrap();

        assert_eq!(response.jwt, "upload-token-123");
        assert_eq!(response.buckets.len(), 2);
        assert_eq!(response.buckets[0].len(), 2);
        assert_eq!(response.buckets[1].len(), 1);
        assert_eq!(response.buckets[0][0], "08f1dfda4574284ab3c21666d1");
        assert_eq!(response.buckets[0][1], "4f1c1af44620d531446ceef93f");
        assert_eq!(response.buckets[1][0], "54995e302614e0523757a04ec1");
    }
}
