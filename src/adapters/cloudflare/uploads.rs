use bon::Builder;
use derive_getters::Getters;
use serde::{Deserialize, Serialize};

use crate::{adapters::cloudflare::deployments::Binding, fs::wrangler::Wrangler};

// this is probably dead because we bundle the upload,
// which we won't do in the future. 
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadSessionResponse {
    pub jwt: String,
    // Contains a list of lists of file hashes that Cloudflare wants us to upload together
    pub buckets: Vec<Vec<String>>,
}

// this is probably dead because we bundle the upload,
// which we won't do in the future. 
#[allow(dead_code)]
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

impl UploadVersionRequest {
    fn create_durable_object_bindings(wrangler: &Wrangler) -> Vec<Binding> {
        wrangler
            .durable_objects()
            .as_ref()
            .map(|durable_objects| {
                durable_objects
                    .iter()
                    .map(|durable_object| Binding::DurableObjectNamespace {
                        name: durable_object.name.clone(),
                        class_name: Some(durable_object.class_name.clone()),
                        script_name: durable_object.script_name.clone(),
                        environment: durable_object.environment.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn create_kv_namespace_bindings(wrangler: &Wrangler) -> Vec<Binding> {
        wrangler
            .kv_namespaces()
            .as_ref()
            .map(|kv_namespaces| {
                kv_namespaces
                    .iter()
                    .map(|kv_namespace| Binding::KvNamespace {
                        name: kv_namespace.binding.clone(),
                        namespace_id: kv_namespace.id.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn create_r2_bucket_bindings(wrangler: &Wrangler) -> Vec<Binding> {
        wrangler
            .r2_buckets()
            .as_ref()
            .map(|r2_buckets| {
                r2_buckets
                    .iter()
                    .map(|r2_bucket| Binding::R2Bucket {
                        name: r2_bucket.binding.clone(),
                        bucket_name: r2_bucket.bucket_name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn create_vectorize_bindings(wrangler: &Wrangler) -> Vec<Binding> {
        wrangler
            .vectorize()
            .as_ref()
            .map(|vectorize_bindings| {
                vectorize_bindings
                    .iter()
                    .map(|vectorize| Binding::Vectorize {
                        name: vectorize.binding.clone(),
                        index_name: vectorize.index_name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn create_service_bindings(wrangler: &Wrangler) -> Vec<Binding> {
        wrangler
            .services()
            .as_ref()
            .map(|services| {
                services
                    .iter()
                    .map(|service| Binding::Service {
                        name: service.binding.clone(),
                        service: service.service.clone(),
                        environment: service.environment.clone().unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn create_tail_consumer_bindings(wrangler: &Wrangler) -> Vec<Binding> {
        wrangler
            .tail_consumers()
            .as_ref()
            .map(|tail_consumers| {
                tail_consumers
                    .iter()
                    .map(|tail_consumer| Binding::TailConsumer {
                        name: tail_consumer.service.clone(),
                        service: tail_consumer.service.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn create_var_bindings(wrangler: &Wrangler) -> Vec<Binding> {
        // Vars in the Wrangler file are Environment variables, but are called PlainText for bindings
        wrangler
            .vars()
            .as_ref()
            .map(|vars| {
                vars.iter()
                    .map(|(key, value)| Binding::PlainText {
                        name: key.clone(),
                        text: value.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl From<Wrangler> for UploadVersionRequest {
    fn from(wrangler: Wrangler) -> Self {
        let mut bindings = Vec::new();

        // The Wrangler file and the UploadVersionRequest share the same data
        // but in a different format, so we need to convert between the two.

        bindings.extend(Self::create_durable_object_bindings(&wrangler));
        bindings.extend(Self::create_kv_namespace_bindings(&wrangler));
        bindings.extend(Self::create_r2_bucket_bindings(&wrangler));
        bindings.extend(Self::create_vectorize_bindings(&wrangler));
        bindings.extend(Self::create_service_bindings(&wrangler));
        bindings.extend(Self::create_tail_consumer_bindings(&wrangler));
        bindings.extend(Self::create_var_bindings(&wrangler));

        Self {
            main_module: wrangler.main().to_string(),
            compatibility_date: wrangler.compatibility_date().clone(),
            compatibility_flags: wrangler.compatibility_flags().clone(),
            bindings: Some(bindings),
        }
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
