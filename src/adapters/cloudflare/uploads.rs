use serde::{Deserialize, Serialize};

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
    pub main_module: String,
}

impl UploadVersionRequest {
    pub fn new(main_module: String) -> Self {
        Self { main_module }
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
