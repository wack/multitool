use bon::Builder;
use derive_getters::Getters;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Builder, Getters)]
pub struct UploadRequest {
    pub manifest: HashMap<String, FileMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileMetadata {
    pub hash: String,
    pub size: u64,
}

impl UploadRequest {
    pub fn new() -> Self {
        Self {
            manifest: HashMap::new(),
        }
    }

    pub fn add_file(&mut self, path: String, hash: String, size: u64) {
        self.manifest.insert(path, FileMetadata { hash, size });
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UploadVersionRequest {
    pub assets: Assets,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Assets {
    pub jwt: String,
}

impl UploadVersionRequest {
    pub fn new(jwt: String) -> Self {
        Self {
            assets: Assets { jwt },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    #[test]
    fn test_serialization() {
        let mut request = UploadRequest::new();
        request.add_file("foo".to_string(), "abc123".to_string(), 1);

        let json = serde_json::to_value(&request).unwrap();
        let expected = json!({
            "manifest": {
                "foo": {
                    "hash": "abc123",
                    "size": 1
                }
            }
        });

        assert_eq!(json, expected);
    }
}
