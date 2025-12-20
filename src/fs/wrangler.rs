use crate::fs::{DirectoryType, file::StaticFile};
use derive_getters::Getters;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// TODO: Check whether this type is actually needed.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Vars {
    #[serde(flatten)]
    pub variables: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct DurableObject {
    pub name: String,
    pub class_name: String,
    pub script_name: Option<String>,
    pub environment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct KvNamespace {
    pub binding: String,
    pub id: String,
    pub preview_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct R2Bucket {
    pub binding: String,
    pub bucket_name: String,
    pub preview_bucket_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Vectorize {
    pub binding: String,
    pub index_name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Service {
    pub binding: String,
    pub service: String,
    pub environment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Route {
    pub pattern: String,
    pub zone_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct TailConsumer {
    pub service: String,
    pub environment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Observability {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sampling_rate: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Getters, Clone)]
pub struct Wrangler {
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    name: String,
    main: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compatibility_flags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    durable_objects: Option<Vec<DurableObject>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kv_namespaces: Option<Vec<KvNamespace>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    r2_buckets: Option<Vec<R2Bucket>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vectorize: Option<Vec<Vectorize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    services: Option<Vec<Service>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tail_consumers: Option<Vec<TailConsumer>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    routes: Option<Vec<Route>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observability: Option<Observability>,
    #[serde(skip_serializing_if = "Option::is_none")]
    vars: Option<HashMap<String, String>>,
}

const WRANGLER_PREFIX: &str = "wrangler";
const WRANGLER_EXTENSIONS: [&str; 3] = ["toml", "json", "jsonc"];

/// A TOML formatted wrangler file
pub struct TomlWranglerFile;

impl StaticFile for TomlWranglerFile {
    type Data = Wrangler;
    const DIR: DirectoryType = DirectoryType::ApplicationRoot;
    const NAME: &'static str = WRANGLER_PREFIX;
    const EXTENSION: &'static str = WRANGLER_EXTENSIONS[0];
}

/// A JSON formatted wrangler file
pub struct JsonWranglerFile;

impl StaticFile for JsonWranglerFile {
    type Data = Wrangler;
    const DIR: DirectoryType = DirectoryType::ApplicationRoot;
    const NAME: &'static str = WRANGLER_PREFIX;
    const EXTENSION: &'static str = WRANGLER_EXTENSIONS[1];
}

/// A JSONC formatted wrangler file
pub struct JsoncWranglerFile;

impl StaticFile for JsoncWranglerFile {
    type Data = Wrangler;
    const DIR: DirectoryType = DirectoryType::ApplicationRoot;
    const NAME: &'static str = WRANGLER_PREFIX;
    const EXTENSION: &'static str = WRANGLER_EXTENSIONS[2];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toml_wrangler_file_config() {
        assert_eq!(TomlWranglerFile::NAME, "wrangler");
        assert_eq!(TomlWranglerFile::EXTENSION, "toml");
    }

    #[test]
    fn test_json_wrangler_file_config() {
        assert_eq!(JsonWranglerFile::NAME, "wrangler");
        assert_eq!(JsonWranglerFile::EXTENSION, "json");
    }

    #[test]
    fn test_wrangler_serde_json() {
        let wrangler = Wrangler {
            account_id: Some("test-account".to_string()),
            name: "test-worker".to_string(),
            main: "index.js".to_string(),
            compatibility_date: Some("2023-01-01".to_string()),
            compatibility_flags: None,
            durable_objects: None,
            kv_namespaces: None,
            r2_buckets: None,
            vectorize: None,
            services: None,
            tail_consumers: None,
            routes: None,
            observability: Some(Observability {
                enabled: true,
                head_sampling_rate: Some(0.1),
            }),
            vars: Some(HashMap::from([("KEY".to_string(), "value".to_string())])),
        };

        // Test JSON serialization/deserialization
        let json_str =
            serde_json::to_string_pretty(&wrangler).expect("Failed to serialize to JSON");
        let _deserialized: Wrangler =
            serde_json::from_str(&json_str).expect("Failed to deserialize from JSON");
    }

    #[test]
    fn test_wrangler_serde_toml() {
        let wrangler = Wrangler {
            account_id: Some("test-account".to_string()),
            name: "test-worker".to_string(),
            main: "index.js".to_string(),
            compatibility_date: Some("2023-01-01".to_string()),
            compatibility_flags: None,
            durable_objects: None,
            kv_namespaces: None,
            r2_buckets: None,
            vectorize: None,
            services: None,
            tail_consumers: None,
            routes: None,
            observability: Some(Observability {
                enabled: true,
                head_sampling_rate: Some(0.1),
            }),
            vars: Some(HashMap::from([("KEY".to_string(), "value".to_string())])),
        };

        // Test TOML serialization/deserialization
        let toml_str = toml::to_string_pretty(&wrangler).expect("Failed to serialize to TOML");
        let _deserialized: Wrangler =
            toml::from_str(&toml_str).expect("Failed to deserialize from TOML");
    }

    #[test]
    fn test_wrangler_serde_jsonc() {
        const RAW_JSONC: &str = r#"/**
 * For more details on how to configure Wrangler, refer to:
 * https://developers.cloudflare.com/workers/wrangler/configuration/
 */
{
    "$schema": "node_modules/wrangler/config-schema.json",
    "name": "multitool-quickstart",
    "account_id": "986dc7f2976d17c6205288a7a946ef6a",
    "main": "src/index.js",
    "compatibility_date": "2025-11-06",
    "observability": {
        "enabled": true
    }
    /**
     * Smart Placement
     * Docs: https://developers.cloudflare.com/workers/configuration/smart-placement/#smart-placement
     */
    // "placement": { "mode": "smart" }
    /**
     * Bindings
     * Bindings allow your Worker to interact with resources on the Cloudflare Developer Platform, including
     * databases, object storage, AI inference, real-time communication and more.
     * https://developers.cloudflare.com/workers/runtime-apis/bindings/
     */
    /**
     * Environment Variables
     * https://developers.cloudflare.com/workers/wrangler/configuration/#environment-variables
     */
    // "vars": { "MY_VARIABLE": "production_value" }
    /**
     * Note: Use secrets to store sensitive data.
     * https://developers.cloudflare.com/workers/configuration/secrets/
     */
    /**
     * Static Assets
     * https://developers.cloudflare.com/workers/static-assets/binding/
     */
    // "assets": { "directory": "./public/", "binding": "ASSETS" }
    /**
     * Service Bindings (communicate between multiple Workers)
     * https://developers.cloudflare.com/workers/wrangler/configuration/#service-bindings
     */
    // "services": [{ "binding": "MY_SERVICE", "service": "my-service" }]
}"#;

        let observed: Wrangler = serde_json5::from_str(RAW_JSONC).expect("Failed to parse JSONC");

        assert_eq!(observed.name, "multitool-quickstart");
        assert_eq!(
            observed.account_id,
            Some("986dc7f2976d17c6205288a7a946ef6a".to_string())
        );
        assert_eq!(observed.main, "src/index.js");
        assert_eq!(observed.compatibility_date, Some("2025-11-06".to_string()));
        assert!(observed.observability.is_some());
        if let Some(observability) = observed.observability {
            assert!(observability.enabled);
            assert_eq!(observability.head_sampling_rate, None);
        }
    }
}
