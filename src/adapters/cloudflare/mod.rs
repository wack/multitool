use anyhow::{Context, Error, anyhow};
use chrono::DateTime;
use derive_getters::Getters;
use miette::{IntoDiagnostic, Result, miette};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::multipart::Part;
use reqwest::{Client, multipart};
use rustc_hash::FxHashMap;
use std::collections::{HashMap, HashSet};
use std::path;
use std::sync::OnceLock;
use swc_bundler::{Bundler, Config, Hook, Load, ModuleData, ModuleRecord};
use swc_common::{FileName, FilePathMapping, GLOBALS, Globals, SourceMap, Span, sync::Lrc};
use swc_core::ecma::loader::resolvers::node::NodeModulesResolver;
use swc_ecma_ast::{EsVersion, KeyValueProp};
use swc_ecma_codegen::{Emitter, text_writer::JsWriter};
use swc_ecma_loader::TargetEnv;
use swc_ecma_parser::parse_file_as_module;
use swc_ecma_parser::{Syntax, TsSyntax};
use tracing::{debug, error};
use uploads::{UploadVersionRequest, UploadVersionResponse};
use url::Url;

use deployments::{CreateDeploymentRequest, DeploymentResponse};
use metrics::MetricsResponse;
use responses::CloudflareResponse;
use routes::{CloudflareRoute, CreateCloudflareRouteRequest, UpdateCloudflareRouteRequest};

use crate::artifacts::CloudflareFileManifest;
use crate::fs::wrangler::{Route, Wrangler};

static URL: OnceLock<Url> = OnceLock::new();

fn init_url() -> Url {
    // NOTE: The trailing slash is important here, otherwise the URL parsing will fail!
    Url::parse("https://api.cloudflare.com/client/v4/").unwrap()
}

#[derive(Clone, Getters)]
pub struct CloudflareClient {
    client: Client,
    /// This is the Cloudflare account id
    account_id: String,
    /// The name of the Cloudflare worker
    worker_name: String,
}

impl CloudflareClient {
    pub fn new(account_id: String, worker_name: String, token: &str) -> Self {
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

        Self {
            client,
            account_id,
            worker_name,
        }
    }

    // Commented out until we verify if we need an upload session.
    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/assets/subresources/upload/methods/create/
    // pub async fn create_assets_upload_session(
    //     &self,
    //     manifest: &CloudflareManifest,
    // ) -> Result<UploadSessionResponse> {
    //     debug!("Creating assets upload session");
    //     let account_id = &self.account_id;
    //     let worker_name = &self.worker_name;
    //     let path =
    //         format!("accounts/{account_id}/workers/scripts/{worker_name}/assets-upload-session");
    //     let url = Self::url_with_path(&path);

    //     let response = self
    //         .client
    //         .post(url)
    //         .json(manifest)
    //         .send()
    //         .await
    //         .into_diagnostic()?;

    //     if !response.status().is_success() {
    //         return Err(miette!(
    //             "Failed to create assets upload session. Error: {:?}",
    //             response.json::<serde_json::Value>().await
    //         ));
    //     }

    //     let upload_session_response = response
    //         .json::<CloudflareResponse<UploadSessionResponse>>()
    //         .await
    //         .into_diagnostic()?;

    //     debug!("Assets upload session created successfully");
    //     Ok(upload_session_response.result)
    // }

    // Commented out until we verify if we need an upload session.
    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/assets/subresources/upload/methods/create/
    // pub async fn upload_assets(
    //     &self,
    //     file_upload_jwt: &str,
    //     bucket: Vec<String>,
    //     manifest: CloudflareManifest,
    // ) -> Result<UploadAssetsResponse> {
    //     debug!("Uploading assets");
    //     let account_id = &self.account_id;
    //     let path = format!("accounts/{account_id}/workers/assets/upload?base64=true");
    //     let url = Self::url_with_path(&path);

    //     let mut request = multipart::Form::new();

    //     let mut files = manifest.files().clone();
    //     files.sort_by_key(|file| file.digest());
    //     // Loops over the file hashes from the bucket CF returns
    //     // Looks up the file in the manifest and adds the base64 content to the request
    //     for file_hash in bucket {
    //         let file_idx = files
    //             .binary_search_by_key(&file_hash, |file| file.digest())
    //             .map_err(|_| miette!("File hash {file_hash} not found in manifest"))?;
    //         let path = files[file_idx].path().to_path_buf();
    //         files.remove(file_idx);

    //         let mut file_bytes = Vec::new();
    //         read_file_as_b64(path, &mut file_bytes).await?;

    //         let file_len = file_bytes.len() as u64;

    //         request = request.part(file_hash, Part::stream_with_length(file_bytes, file_len));
    //     }

    //     let mut file_upload_headers = HeaderMap::new();

    //     // Set the authorization header with the JWT token we got from the session upload request
    //     let file_upload_auth = format!("Bearer {file_upload_jwt}");
    //     let mut file_upload_jwt_header = HeaderValue::from_str(&file_upload_auth).unwrap();
    //     file_upload_jwt_header.set_sensitive(true);
    //     file_upload_headers.insert(AUTHORIZATION, file_upload_jwt_header);

    //     let response = self
    //         .client
    //         .post(url.clone())
    //         // TODO: during testing, check if this overrides the default headers
    //         .headers(file_upload_headers)
    //         .multipart(request)
    //         .send()
    //         .await
    //         .into_diagnostic()?;

    //     if !response.status().is_success() {
    //         return Err(miette!(
    //             "Failed to upload asset(s). Error: {:?}",
    //             response.json::<serde_json::Value>().await
    //         ));
    //     }

    //     let upload_response = response
    //         .json::<CloudflareResponse<UploadAssetsResponse>>()
    //         .await
    //         .into_diagnostic()?;

    //     debug!("Assets uploaded successfully");
    //     Ok(upload_response.result)
    // }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/versions/methods/create/
    pub async fn upload_version(
        &self,
        manifest: &CloudflareFileManifest,
        wrangler: Wrangler,
    ) -> Result<UploadVersionResponse> {
        debug!("Uploading Worker version");
        let account_id = &self.account_id;
        let worker_name = &self.worker_name;
        let path = format!("accounts/{account_id}/workers/scripts/{worker_name}/versions");
        let url = Self::url_with_path(&path);

        // Convert our wrangler file to the Request format Cloudflare expects
        let bundled_file = bundle_files(manifest, &wrangler)?;
        let upload_version_request = UploadVersionRequest::from(wrangler.clone());

        let mut request = multipart::Form::new().text(
            "metadata",
            serde_json::to_string(&upload_version_request).into_diagnostic()?,
        );

        // Add the bundled file to the upload request
        let main_file_name = wrangler.main().to_string();
        let mut file_part = Part::bytes(bundled_file);
        file_part = file_part.file_name(main_file_name.clone());
        file_part = file_part.mime_str("application/javascript+module").unwrap();
        request = request.part(main_file_name.clone(), file_part);
        debug!("Added bundled file to upload request: {}", main_file_name);

        debug!("Making upload files request");
        let response = self
            .client
            .post(url)
            .multipart(request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to upload Worker version. Error: {:?}",
                response
                    .json::<CloudflareResponse<serde_json::Value>>()
                    .await
            ));
        }

        let version_response = response
            .json::<CloudflareResponse<UploadVersionResponse>>()
            .await
            .into_diagnostic()?;

        debug!("Worker version uploaded successfully");
        Ok(version_response.result)
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/get/
    pub async fn get_current_version(&self) -> Result<String> {
        debug!("Getting current Worker version");
        let account_id = &self.account_id;
        let worker_name = &self.worker_name;
        let path = format!("accounts/{account_id}/workers/scripts/{worker_name}/deployments");
        let url = Self::url_with_path(&path);

        let response = self.client.get(url).send().await.into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to get current Worker version. Error: {:?}",
                response.json::<serde_json::Value>().await
            ));
        }

        let deployment_response = response
            .json::<CloudflareResponse<DeploymentResponse>>()
            .await
            .into_diagnostic()?;

        debug!("Current Worker version retrieved successfully");
        // The cloudflare API auto-sorts deployments so the first deployment listed is the currently active deployment
        let version_id = deployment_response
            .result
            .deployments
            .first()
            .ok_or(miette!("No deployments found"))?
            .versions
            .first()
            .ok_or(miette!("No deployment versions found"))?
            .version_id
            .clone();

        Ok(version_id)
    }

    // Corresponds to:
    // https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/deployments/methods/create/
    pub async fn create_deployment(&self, request: CreateDeploymentRequest) -> Result<()> {
        debug!("Deploying updated version(s)");
        let account_id = &self.account_id;
        let worker_name = &self.worker_name;
        let path = format!("accounts/{account_id}/workers/scripts/{worker_name}/deployments");
        let url = Self::url_with_path(&path);

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to create new Worker deployment. Error: {:?}",
                response.json::<serde_json::Value>().await
            ));
        }
        debug!("Worker deployment successful");
        Ok(())
    }

    // For the monitor to grab metrics within a time range.
    pub async fn collect_metrics(
        &self,
        worker_version_id: String,
        status_code_range_start: u16,
        status_code_range_end: u16,
        from_time: DateTime<chrono::Utc>,
        to_time: DateTime<chrono::Utc>,
    ) -> Result<u32> {
        let account_id = &self.account_id;
        let worker_name = &self.worker_name;
        let path = format!("accounts/{account_id}/workers/observability/telemetry/query");
        let url = Self::url_with_path(&path);

        // Convert DateTime to Unix Millisecond timestamps
        let from_timestamp = from_time.timestamp_millis() as u64;
        let to_timestamp = to_time.timestamp_millis() as u64;

        let query_body = serde_json::json!({
          "view": "calculations",
          "queryId": "worker-calculation",
          "parameters": {
            "datasets": [
              "cloudflare-workers"
            ],
            "filters": [
              {
                "key": "$metadata.service",
                "operation": "eq",
                "type": "string",
                "value": worker_name
              },
              {
                "key": "$workers.scriptVersion.id",
                "operation": "eq",
                "type": "string",
                "value": worker_version_id
              },
              {
                "key": "$workers.event.response.status",
                "operation": "gte",
                "type": "number",
                "value": status_code_range_start
              },
              {
                "key": "$workers.event.response.status",
                "operation": "lte",
                "type": "number",
                "value": status_code_range_end
              }
            ],
            "calculations": [
              {
                "key": "$workers.event.response.status",
                "operator": "count",
                "keyType": "number",
                "alias": "sum"
              }
            ]
          },
          "timeframe": {
            "to": to_timestamp,
            "from": from_timestamp
          }
        });

        let response = self
            .client
            .post(url)
            .json(&query_body)
            .send()
            .await
            .into_diagnostic()?;

        // If there's an error, just return 0 results
        if !response.status().is_success() {
            error!(
                "Failed to query Cloudflare metrics for worker: {}, version: {}, status codes: {}-{}, error: {:?}",
                worker_name,
                worker_version_id,
                status_code_range_start,
                status_code_range_end,
                response.json::<serde_json::Value>().await
            );
            return Ok(0);
        }

        let metrics_response = response
            .json::<CloudflareResponse<MetricsResponse>>()
            .await
            .into_diagnostic()?;

        let count = metrics_response
            .result
            .calculations
            .get(0)
            .and_then(|c| c.aggregates.get(0).cloned())
            .map_or(0, |a| a.count);

        Ok(count)
    }

    // Updates the routes from wrangler.toml with Cloudflare
    pub async fn update_routes(&self, wrangler_routes: Vec<Route>) -> Result<()> {
        debug!("Updating routes in Cloudflare");

        let routes_by_zone = group_routes(&wrangler_routes);

        // For each zone, compare the current routes with the wrangler routes
        // and update, create, or delete routes as necessary
        for (zone_id, zone_routes) in routes_by_zone {
            let cf_routes = self.get_routes(&zone_id).await?;

            let wrangler_patterns: HashSet<&String> =
                zone_routes.iter().map(|r| &r.pattern).collect();
            let cf_routes_map: HashMap<&String, &CloudflareRoute> =
                cf_routes.iter().map(|r| (&r.pattern, r)).collect();

            // Update or create routes from wrangler_routes
            for wrangler_route in zone_routes {
                if let Some(cf_route) = cf_routes_map.get(&wrangler_route.pattern) {
                    // Route exists, check if the pattern needs updating
                    if cf_route.pattern != wrangler_route.pattern {
                        self.update_route(&zone_id, &cf_route.id, &wrangler_route.pattern)
                            .await?;
                    }
                } else {
                    // Route doesn't exist, create it
                    self.create_route(&zone_id, &wrangler_route.pattern).await?;
                }
            }

            // Finally, delete a route if it exists in Cloudflare but not in wrangler_routes
            for cf_route in &cf_routes {
                if !wrangler_patterns.contains(&cf_route.pattern)
                    && cf_route.script.as_ref() == Some(&self.worker_name)
                {
                    self.delete_route(&zone_id, &cf_route.id).await?;
                }
            }
        }

        debug!("Routes updated successfully!");
        Ok(())
    }

    async fn get_routes(&self, zone_id: &str) -> Result<Vec<CloudflareRoute>> {
        debug!("Getting routes for zone: {}", zone_id);
        let path = format!("zones/{}/workers/routes", zone_id);
        let url = Self::url_with_path(&path);

        let response = self.client.get(url).send().await.into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to get routes for zone {}. Error: {:?}",
                zone_id,
                response.json::<serde_json::Value>().await
            ));
        }

        let routes_response = response
            .json::<CloudflareResponse<Vec<CloudflareRoute>>>()
            .await
            .into_diagnostic()?;

        Ok(routes_response.result)
    }

    async fn create_route(&self, zone_id: &str, pattern: &str) -> Result<CloudflareRoute> {
        debug!(
            "Creating route with pattern: {} for zone: {}",
            pattern, zone_id
        );
        let path = format!("zones/{}/workers/routes", zone_id);
        let url = Self::url_with_path(&path);

        let request = CreateCloudflareRouteRequest {
            pattern: pattern.to_string(),
            script: self.worker_name.clone(),
        };

        let response = self
            .client
            .post(url)
            .json(&request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to create route with pattern {}. Error: {:?}",
                pattern,
                response.json::<serde_json::Value>().await
            ));
        }

        let route_response = response
            .json::<CloudflareResponse<CloudflareRoute>>()
            .await
            .into_diagnostic()?;

        debug!("Route created successfully with pattern: {}!", pattern);
        Ok(route_response.result)
    }

    async fn update_route(
        &self,
        zone_id: &str,
        route_id: &str,
        pattern: &str,
    ) -> Result<CloudflareRoute> {
        debug!(
            "Updating route {} with pattern: {} for zone: {}",
            route_id, pattern, zone_id
        );
        let path = format!("zones/{}/workers/routes/{}", zone_id, route_id);
        let url = Self::url_with_path(&path);

        let request = UpdateCloudflareRouteRequest {
            pattern: pattern.to_string(),
            script: self.worker_name.clone(),
        };

        let response = self
            .client
            .put(url)
            .json(&request)
            .send()
            .await
            .into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to update route {}. Error: {:?}",
                route_id,
                response.json::<serde_json::Value>().await
            ));
        }

        let route_response = response
            .json::<CloudflareResponse<CloudflareRoute>>()
            .await
            .into_diagnostic()?;

        debug!("Route {} updated successfully!", route_id);
        Ok(route_response.result)
    }

    async fn delete_route(&self, zone_id: &str, route_id: &str) -> Result<()> {
        debug!("Deleting route: {} for zone: {}", route_id, zone_id);
        let path = format!("zones/{}/workers/routes/{}", zone_id, route_id);
        let url = Self::url_with_path(&path);

        let response = self.client.delete(url).send().await.into_diagnostic()?;

        if !response.status().is_success() {
            return Err(miette!(
                "Failed to delete route {}. Error: {:?}",
                route_id,
                response.json::<serde_json::Value>().await
            ));
        }

        debug!("Route {} deleted successfully!", route_id);
        Ok(())
    }

    fn base_url() -> &'static Url {
        URL.get_or_init(init_url)
    }

    /// Adds the URL path to the base URL.
    /// All paths MUST NOT start with a slash.
    fn url_with_path(path: &str) -> Url {
        let url = Self::base_url().clone();
        url.join(path).unwrap()
    }
}

/// Groups routes by zone_id so they can be processed in batches since
/// each route could have a different zone_id
fn group_routes(wrangler_routes: &[Route]) -> HashMap<String, Vec<&Route>> {
    let mut routes_by_zone: HashMap<String, Vec<&Route>> = HashMap::new();
    for route in wrangler_routes {
        if let Some(zone_id) = &route.zone_id {
            routes_by_zone
                .entry(zone_id.clone())
                .or_default()
                .push(route);
        }
    }
    routes_by_zone
}

// / Determines the appropriate syntax parser based on file extension
fn determine_syntax_for_file(file: &FileName) -> Syntax {
    let path_str = match file {
        FileName::Real(path) => path.to_string_lossy(),
        FileName::Custom(name) => std::borrow::Cow::Borrowed(name.as_str()),
        _ => return Syntax::Es(Default::default()), // Default fallback
    };

    // Check extension
    if let Some(ext_start) = path_str.rfind('.') {
        let ext = &path_str[ext_start + 1..];
        match ext {
            "ts" | "tsx" | "mts" | "cts" => Syntax::Typescript(TsSyntax::default()),
            "js" | "jsx" | "mjs" | "cjs" => Syntax::Es(Default::default()),
            _ => {
                // For unknown extensions, check if it's in node_modules (use JS) or project files (use TS)
                if path_str.contains("node_modules") {
                    Syntax::Es(Default::default())
                } else {
                    Syntax::Typescript(TsSyntax::default())
                }
            }
        }
    } else {
        // No extension - default based on location
        if path_str.contains("node_modules") {
            Syntax::Es(Default::default())
        } else {
            Syntax::Typescript(TsSyntax::default())
        }
    }
}

fn bundle_files(manifest: &CloudflareFileManifest, wrangler: &Wrangler) -> Result<Vec<u8>> {
    let globals = Globals::new();
    let cm = Lrc::new(SourceMap::new(FilePathMapping::empty()));

    // Build external modules list using SWC's official Node.js built-ins
    let external_modules = build_external_modules_list();

    GLOBALS.set(&globals, || {
        let mut bundler = Bundler::new(
            &globals,
            cm.clone(),
            FileSystemLoader { cm: cm.clone() },
            NodeModulesResolver::new(TargetEnv::Node, FxHashMap::default(), false),
            Config {
                require: true,
                external_modules,
                module: swc_bundler::ModuleType::Es,
                ..Default::default()
            },
            Box::new(Noop),
        );

        // Find entry points from the manifest files
        let mut entries = HashMap::default();

        // Use the main module specified in the Wrangler configuration
        let main_module = wrangler.main();
        let mut entry_found = false;

        for file_path in manifest.files() {
            let relative_path = file_path.to_str().unwrap_or("");

            // Check if this file matches the main module from wrangler
            if relative_path.ends_with(main_module) || relative_path == main_module {
                entries.insert("main".to_string(), FileName::Real(file_path.clone()));
                entry_found = true;
                break;
            }
        }

        // If the specified main module is not found, return an error
        if !entry_found {
            return Err(miette!(
                "Main module '{}' specified in wrangler configuration not found in manifest files",
                main_module
            ));
        }

        let mut bundles = bundler
            .bundle(entries)
            .map_err(|e| miette!("Failed to bundle files: {:?}", e))?;

        if bundles.is_empty() {
            return Err(miette!("No bundles were generated"));
        }

        let bundle = bundles.pop().unwrap();

        // Create an in-memory buffer to capture the emitted code
        let mut output_buffer = Vec::new();
        let mut emitter = Emitter {
            cfg: swc_ecma_codegen::Config::default(),
            cm: cm.clone(),
            comments: None,
            wr: Box::new(JsWriter::new(cm, "\n", &mut output_buffer, None)),
        };

        emitter
            .emit_module(&bundle.module)
            .map_err(|e| miette!("Failed to emit bundled module: {:?}", e))?;

        // TODO: REMOVE BEFORE PRODUCTION
        std::fs::write("bundle.js", &output_buffer)
            .map_err(|e| miette!("Failed to write debug bundle file: {:?}", e))?;
        debug!("Bundled file saved to bundle.js for debugging");

        Ok(output_buffer)
    })
}

/// A robust filesystem-based loader inspired by SWC's official examples
/// This handles both real files and custom files with better error handling
struct FileSystemLoader {
    cm: Lrc<SourceMap>,
}

impl Load for FileSystemLoader {
    fn load(&self, file: &FileName) -> Result<ModuleData, Error> {
        debug!("Loading file: {}", file);

        let fm = match file {
            FileName::Real(path) => self
                .cm
                .load_file(path)
                .with_context(|| format!("Failed to load file: {}", path.display()))?,
            FileName::Custom(name) => {
                debug!("Loading custom file: {}", name);

                // Handle Node.js built-in modules (should be marked as external)
                if name.starts_with("node:") || is_node_builtin(name) {
                    return Err(anyhow!(
                        "Node.js built-in module should be external: {}",
                        name
                    ));
                }

                // Try to resolve the custom name as a file path
                let path_buf = path::PathBuf::from(name);
                if path_buf.exists() {
                    self.cm
                        .load_file(&path_buf)
                        .with_context(|| format!("Failed to load custom file: {}", name))?
                } else {
                    return Err(anyhow!("Custom file not found: {}", name));
                }
            }
            _ => {
                return Err(anyhow!("Unsupported file name type: {:?}", file));
            }
        };

        // Determine the appropriate syntax parser based on file extension
        let syntax = determine_syntax_for_file(file);

        // Parse the module with better error handling
        let module = parse_file_as_module(&fm, syntax, EsVersion::Es2020, None, &mut Vec::new())
            .map_err(|err| anyhow!("Failed to parse module {}: {:?}", file, err))?;

        Ok(ModuleData {
            fm,
            module,
            helpers: Default::default(),
        })
    }
}

/// Check if a module name is a Node.js built-in
fn is_node_builtin(name: &str) -> bool {
    // Use swc_ecma_loader's NODE_BUILTINS constant for comprehensive list
    swc_ecma_loader::NODE_BUILTINS.contains(&name)
}

/// Build a comprehensive list of external modules for the bundler
/// This includes Node.js built-ins and other modules that should remain external
fn build_external_modules_list() -> Vec<swc_atoms::Atom> {
    let mut external_modules = Vec::new();

    // Add all Node.js built-in modules from SWC's official list
    for &builtin in swc_ecma_loader::NODE_BUILTINS {
        // Add the module name as-is (legacy format)
        external_modules.push(swc_atoms::Atom::from(builtin));

        // Add the module with node: prefix (modern format)
        external_modules.push(swc_atoms::Atom::from(format!("node:{}", builtin).as_str()));
    }

    // Remove duplicates and sort for consistency
    external_modules.sort();
    external_modules.dedup();

    debug!(
        "Built external modules list with {} entries",
        external_modules.len()
    );

    external_modules
}

struct Noop;

impl Hook for Noop {
    fn get_import_meta_props(&self, _: Span, _: &ModuleRecord) -> Result<Vec<KeyValueProp>, Error> {
        unimplemented!()
    }
}
pub mod deployments;
mod metrics;
mod responses;
mod routes;
pub mod uploads;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::wrangler::Route;
    use swc_common::FileName;

    #[test]
    fn test_determine_syntax_typescript_files() {
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("file.ts".into())),
            Syntax::Typescript(_)
        ));
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("file.tsx".into())),
            Syntax::Typescript(_)
        ));
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("file.mts".into())),
            Syntax::Typescript(_)
        ));
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("file.cts".into())),
            Syntax::Typescript(_)
        ));
    }

    #[test]
    fn test_determine_syntax_javascript_files() {
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("file.js".into())),
            Syntax::Es(_)
        ));
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("file.jsx".into())),
            Syntax::Es(_)
        ));
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("file.mjs".into())),
            Syntax::Es(_)
        ));
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("file.cjs".into())),
            Syntax::Es(_)
        ));
    }

    #[test]
    fn test_determine_syntax_node_modules_defaults_to_js() {
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("node_modules/package/index.unknown".into())),
            Syntax::Es(_)
        ));
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("./node_modules/package/file.xyz".into())),
            Syntax::Es(_)
        ));
    }

    #[test]
    fn test_determine_syntax_unknown_extension_defaults_to_ts() {
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("file.unknown".into())),
            Syntax::Typescript(_)
        ));
        assert!(matches!(
            determine_syntax_for_file(&FileName::Real("src/components/App.vue".into())),
            Syntax::Typescript(_)
        ));
    }

    #[test]
    fn test_group_routes_empty() {
        let routes = vec![];
        let result = group_routes(&routes);
        assert!(result.is_empty());
    }

    #[test]
    fn test_group_routes_single_zone() {
        let routes = vec![
            Route {
                pattern: "example.com/*".to_string(),
                zone_id: Some("zone1".to_string()),
            },
            Route {
                pattern: "example.com/api/*".to_string(),
                zone_id: Some("zone1".to_string()),
            },
        ];
        let result = group_routes(&routes);
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("zone1"));
        assert_eq!(result["zone1"].len(), 2);
        assert_eq!(result["zone1"][0].pattern, "example.com/*");
        assert_eq!(result["zone1"][1].pattern, "example.com/api/*");
    }

    #[test]
    fn test_group_routes_multiple_zones() {
        let routes = vec![
            Route {
                pattern: "example.com/*".to_string(),
                zone_id: Some("zone1".to_string()),
            },
            Route {
                pattern: "test.com/*".to_string(),
                zone_id: Some("zone2".to_string()),
            },
            Route {
                pattern: "example.com/api/*".to_string(),
                zone_id: Some("zone1".to_string()),
            },
        ];
        let result = group_routes(&routes);
        assert_eq!(result.len(), 2);
        assert!(result.contains_key("zone1"));
        assert!(result.contains_key("zone2"));
        assert_eq!(result["zone1"].len(), 2);
        assert_eq!(result["zone2"].len(), 1);
        assert_eq!(result["zone1"][0].pattern, "example.com/*");
        assert_eq!(result["zone1"][1].pattern, "example.com/api/*");
        assert_eq!(result["zone2"][0].pattern, "test.com/*");
    }

    #[test]
    fn test_parse_module_specifier_scoped_package() {
        // Test parsing logic for scoped packages (this would be part of PathResolver)
        let module_specifier = "@babel/core";
        let (package_name, subpath) = if module_specifier.starts_with('@') {
            let parts: Vec<&str> = module_specifier.splitn(3, '/').collect();
            if parts.len() >= 2 {
                let package_name = format!("{}/{}", parts[0], parts[1]);
                let subpath = if parts.len() > 2 {
                    Some(parts[2])
                } else {
                    None
                };
                (package_name, subpath)
            } else {
                (module_specifier.to_string(), None)
            }
        } else {
            let parts: Vec<&str> = module_specifier.splitn(2, '/').collect();
            let package_name = parts[0].to_string();
            let subpath = if parts.len() > 1 {
                Some(parts[1])
            } else {
                None
            };
            (package_name, subpath)
        };

        assert_eq!(package_name, "@babel/core");
        assert_eq!(subpath, None);
    }

    #[test]
    fn test_parse_module_specifier_scoped_package_with_subpath() {
        let module_specifier = "@babel/core/lib/config";
        let (package_name, subpath) = if module_specifier.starts_with('@') {
            let parts: Vec<&str> = module_specifier.splitn(3, '/').collect();
            if parts.len() >= 2 {
                let package_name = format!("{}/{}", parts[0], parts[1]);
                let subpath = if parts.len() > 2 {
                    Some(parts[2])
                } else {
                    None
                };
                (package_name, subpath)
            } else {
                (module_specifier.to_string(), None)
            }
        } else {
            let parts: Vec<&str> = module_specifier.splitn(2, '/').collect();
            let package_name = parts[0].to_string();
            let subpath = if parts.len() > 1 {
                Some(parts[1])
            } else {
                None
            };
            (package_name, subpath)
        };

        assert_eq!(package_name, "@babel/core");
        assert_eq!(subpath, Some("lib/config"));
    }

    #[test]
    fn test_parse_module_specifier_regular_package() {
        let module_specifier = "lodash";
        let (package_name, subpath) = if module_specifier.starts_with('@') {
            let parts: Vec<&str> = module_specifier.splitn(3, '/').collect();
            if parts.len() >= 2 {
                let package_name = format!("{}/{}", parts[0], parts[1]);
                let subpath = if parts.len() > 2 {
                    Some(parts[2])
                } else {
                    None
                };
                (package_name, subpath)
            } else {
                (module_specifier.to_string(), None)
            }
        } else {
            let parts: Vec<&str> = module_specifier.splitn(2, '/').collect();
            let package_name = parts[0].to_string();
            let subpath = if parts.len() > 1 {
                Some(parts[1])
            } else {
                None
            };
            (package_name, subpath)
        };

        assert_eq!(package_name, "lodash");
        assert_eq!(subpath, None);
    }

    #[test]
    fn test_parse_module_specifier_regular_package_with_subpath() {
        let module_specifier = "lodash/fp/get";
        let (package_name, subpath) = if module_specifier.starts_with('@') {
            let parts: Vec<&str> = module_specifier.splitn(3, '/').collect();
            if parts.len() >= 2 {
                let package_name = format!("{}/{}", parts[0], parts[1]);
                let subpath = if parts.len() > 2 {
                    Some(parts[2])
                } else {
                    None
                };
                (package_name, subpath)
            } else {
                (module_specifier.to_string(), None)
            }
        } else {
            let parts: Vec<&str> = module_specifier.splitn(2, '/').collect();
            let package_name = parts[0].to_string();
            let subpath = if parts.len() > 1 {
                Some(parts[1])
            } else {
                None
            };
            (package_name, subpath)
        };

        assert_eq!(package_name, "lodash");
        assert_eq!(subpath, Some("fp/get"));
    }
}
