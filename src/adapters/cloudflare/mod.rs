use chrono::DateTime;
use derive_getters::Getters;
use miette::{IntoDiagnostic, Result, miette};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use reqwest::multipart::Part;
use reqwest::{Client, multipart};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;
use swc_common::comments::SingleThreadedComments;
use swc_common::{GLOBALS, Mark, SourceMap, errors::Handler, sync::Lrc};
use swc_ecma_codegen::to_code_default;
use swc_ecma_parser::{Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_ecma_transforms_base::{fixer::fixer, hygiene::hygiene, resolver};
use swc_ecma_transforms_typescript::strip;
use tokio::fs::read;
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
        let upload_version_request = UploadVersionRequest::from(wrangler);

        let mut request = multipart::Form::new().text(
            "metadata",
            serde_json::to_string(&upload_version_request).into_diagnostic()?,
        );

        for file_path in manifest.files() {
            // Process JavaScript/TypeScript files through SWC transformation
            let file_bytes = if let Some(extension) = file_path.extension() {
                if extension == "js" || extension == "ts" {
                    // Use processed/transformed bytes for JS/TS files
                    process_file(file_path)?
                } else {
                    // Use original bytes for other file types
                    read(file_path).await.into_diagnostic()?
                }
            } else {
                // Ignore files without extensions
                continue;
            };

            let mut file_part = Part::bytes(file_bytes);

            // Ensure we keep the whole file path (with the root stripped) as the name, not just the individual file name
            let file_path_str = file_path.to_str().unwrap().to_string();
            file_part = file_part.file_name(file_path_str.clone());
            file_part = file_part.mime_str("application/javascript+module").unwrap();
            request = request.part(file_path_str, file_part);
        }

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

/// Lexer are scoped by this lifetime because the string they're
/// lexing has to live at least as long as the lexer. This is to
/// allow the lexer to borrow the contents of the file without
/// having to copy them.
fn build_lexer<'a>(input: StringInput<'a>) -> Lexer<'a> {
    Lexer::new(
        Syntax::Typescript(TsSyntax::default()),
        Default::default(),
        input,
        None,
    )
}

/// Processes JavaScript and TypeScript files by lexing, parsing, and transforming them.
/// This function loads the source code, parses it into an AST, applies transformations
/// (removes TypeScript types, fixes hygiene, adds parentheses), and returns the transformed code as bytes.
fn process_file(file: &Path) -> Result<Vec<u8>> {
    // Lrc is just SWC's wrapper around Rust's standard Rc/Arc type.
    // They swap between the two based on whether you enable parallel compilation or not.
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(
        swc_common::errors::ColorConfig::Auto,
        true,
        false,
        Some(cm.clone()),
    );

    // Load the source code as a "file" from disk.
    let source_file = cm
        .load_file(file)
        .expect("File must exist and be readable.");
    // Read it into a type that can be lexed/parsed. Basically
    // an in-memory buffer of bytes.
    let source_input = StringInput::from(&*source_file);
    // Builder a lexer and a parser for the file.
    let lexer = build_lexer(source_input);
    let mut parser = Parser::new_from(lexer);

    // Dump any errors.
    for e in parser.take_errors() {
        e.into_diagnostic(&handler).emit();
    }

    // Parse the source code into a module.
    let module = parser
        .parse_program()
        .map_err(|e| e.into_diagnostic(&handler).emit())
        .expect("failed to parse module.");

    let comments = SingleThreadedComments::default();

    // Finally, transform the source code.
    // We have to enter this bizarre "globals" context to save
    // span information about this file. This is just some weirdness
    // in the SWC API from what I can tell. I'm sure there's a more
    // hygenic way to do this, but this is what was in the example.
    let globals = Default::default();
    let transformed_code = GLOBALS.set(&globals, || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();

        // Conduct identifier scope analysis
        let module = module.apply(resolver(unresolved_mark, top_level_mark, true));

        // Remove typescript types
        let module = module.apply(strip(unresolved_mark, top_level_mark));

        // Fix up any identifiers with the same name, but different contexts
        let module = module.apply(hygiene());

        // Ensure that we have enough parenthesis.
        let program = module.apply(fixer(Some(&comments)));

        to_code_default(cm.clone(), Some(&comments), &program)
    });

    Ok(transformed_code.into_bytes())
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
}
