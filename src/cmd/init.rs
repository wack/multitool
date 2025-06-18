//! The `init` subcommand is implemented as a state machine
//! that maniputates stack allocated data with each state.
//! The input is a (potentially empty) manifest file. Each state
//! asks a question of the user to fill in the manifest file,
//! like 20 Questions. As you answer more and more questions, you
//! navigate deeper and deeper into the manifest's structure,
//! until you reach a leaf node. Each leaf node corresponds to a
//! field in the manifest.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bon::Builder;
use miette::{IntoDiagnostic, Result};
use tokio::{runtime::Runtime, sync::Mutex};
use tracing::{info, warn};

use crate::{
    MULTITOOL_ORIGIN, Terminal,
    adapters::{BackendClient, CloudflareClient, backend::WorkspaceId},
    config::InitSubcommand,
    fs::{FileSystem, Session, SessionFile},
    manifest::{
        AwsApiGatewayConfig, AwsCloudwatch, AwsLambdaConfig, CloudflareConfig, IngressConfig,
        InitTomlManifest, Manifest, MonitorConfig, PlatformConfig,
    },
    utils::load_default_aws_config,
};

pub struct Init {
    terminal: Terminal,
    origin: String,
}

impl Init {
    pub fn new(terminal: Terminal, flags: InitSubcommand) -> Result<Self> {
        let origin = flags
            .origin()
            .clone()
            .unwrap_or(MULTITOOL_ORIGIN.to_owned());
        Ok(Self { terminal, origin })
    }

    pub fn dispatch(self) -> Result<()> {
        // Kick off the async runtime.
        let rt = Runtime::new().into_diagnostic()?;
        let _guard = rt.enter();
        rt.block_on(async {
            let fs = FileSystem::new()?;

            // Exit early if there's already a application manifest.
            //
            // For now, we don't allow users to use init to
            // edit their manifest (because we can't preserve
            // comments right Serde right now...)
            // TODO: Upgrade our config-file reading to use
            // the toml and toml_edit crates to preserve comments
            // in the TOML files we read.
            // https://docs.rs/toml_edit/latest/toml_edit/
            if let Ok(_) = fs.application_manifest() {
                info!(
                    "It looks like you already have an initialized application manifest. Exiting."
                );
                return Ok(());
            }
            info!("No application manifest file found. Let's create one!");
            // Create a new manifest instance.
            let manifest = Arc::new(Mutex::new(Manifest::default()));
            let terminal = Arc::new(self.terminal);
            let start = Start::builder()
                .manifest(manifest)
                .fs(fs.clone())
                .terminal(terminal)
                .origin(self.origin.clone())
                .build();
            let mut state = State::Next(Box::new(start));

            let manifest = loop {
                match state {
                    State::Next(mut next) => state = next.run().await,
                    State::Done(manifest) => break manifest,
                    State::Err(report) => return Err(report),
                }
            };
            // Now that we've edited the inner manifest and finalized
            // the value, we can copy the guts out from inside the mutex.
            let manifest_guard = manifest.lock().await;
            let final_manifest = manifest_guard.clone();

            // Dump the file to disk.
            fs.save_file(&InitTomlManifest, &final_manifest)
        })
    }
}

#[derive(Builder)]
struct Start {
    manifest: Arc<Mutex<Manifest>>,
    terminal: Arc<Terminal>,
    origin: String,
    fs: FileSystem,
}

#[async_trait]
impl InitStateMachine for Start {
    type Output = Arc<Mutex<Manifest>>;

    async fn run(&mut self) -> State<Self::Output> {
        let next = CheckLogin::builder()
            .manifest(self.manifest.clone())
            .fs(self.fs.clone())
            .terminal(self.terminal.clone())
            .origin(self.origin.clone())
            .build();
        State::Next(Box::new(next))
    }
}

#[derive(Builder)]
struct CheckLogin {
    manifest: Arc<Mutex<Manifest>>,
    terminal: Arc<Terminal>,
    fs: FileSystem,
    origin: String,
}

impl CheckLogin {
    async fn prompt_login(&self) -> Result<Session> {
        let email = self.terminal.prompt_text("Email");
        let password = self.terminal.prompt_password();
        let backend = BackendClient::new(Some(&self.origin), None)?;
        let creds = backend.exchange_creds(&email, &password).await?;
        self.fs.save_file(&SessionFile, &creds)?;
        self.terminal.login_successful()?;
        info!("Now that you're logged in, let's continue.");
        Ok(creds)
    }
}

impl<T> From<miette::Report> for State<T> {
    fn from(value: miette::Report) -> Self {
        Self::Err(value)
    }
}

#[async_trait]
impl InitStateMachine for CheckLogin {
    type Output = Arc<Mutex<Manifest>>;

    async fn run(&mut self) -> State<Self::Output> {
        // Check to ensure the user is logged in.
        // If they're already logged in, then we can continue
        // forward with the `init` process.
        info!("Checking to see if you're logged in");
        let session = match self.fs.load_file(SessionFile) {
            Ok(session) => Ok(session),
            // If they're not logged in, then we have to log
            // them in and get back a session instance so we
            // can build the Backend client.
            Err(_) => self.prompt_login().await,
        };

        // Now that they're logged in, we can create a backend
        // client using their auth information.
        let origin = Some(self.origin.as_ref());
        let backend = match session {
            Ok(value) => BackendClient::new(origin, Some(value)),
            Err(err) => return State::Err(err),
        };
        // Unwrap the backend client.
        let backend = match backend {
            Ok(backend) => backend,
            Err(err) => return State::Err(err),
        };
        // Now, we can ask the user about which workspace they
        // want to operate on.
        let next = PromptWorkspace::builder()
            .manifest(self.manifest.clone())
            .terminal(self.terminal.clone())
            .fs(self.fs.clone())
            .backend(backend)
            .build();

        State::Next(Box::new(next))
    }
}

#[derive(Builder)]
struct PromptWorkspace {
    manifest: Arc<Mutex<Manifest>>,
    terminal: Arc<Terminal>,
    fs: FileSystem,
    backend: BackendClient,
}

#[async_trait]
impl InitStateMachine for PromptWorkspace {
    type Output = Arc<Mutex<Manifest>>;

    async fn run(&mut self) -> State<Self::Output> {
        debug_assert!(self.backend.is_authenicated().is_ok());
        // Now that we have an authenticated client, we
        // can read their workspaces and ask if they want
        // to use an existing workspace or create a new one.
        let workspaces = match self.backend.list_workspaces().await {
            Ok(workspaces) => workspaces,
            Err(err) => return State::Err(err),
        };
        let workspace_names: Vec<_> = workspaces
            .iter()
            .map(|workspace| workspace.display_name.clone())
            .collect();
        // We're going to prompt the user to pick out their workspace
        // from the list, or to create a new one.
        // To give them that option, we have to add a new element
        // to the list.
        let mut options = workspace_names.clone();
        options.push("+ Create new workspace".to_owned());
        // Now, we can prompt the user to select an option.
        info!("Which workspace would you like to use?");
        let selection = self
            .terminal
            .prompt_single_selection("Workspace", options.as_slice());
        if selection < workspaces.len() {
            // The user has selected an existing workspace.
            let selected_workspace = &workspaces[selection];
            let workspace_name = selected_workspace.display_name.clone();
            let workspace_id = selected_workspace.id;

            let mut manifest_guard = self.manifest.lock().await;
            manifest_guard.set_workspace(workspace_name);
            drop(manifest_guard);
            // Awesome, now we can move on to the application.
            let next = PromptApplication::builder()
                .workspace_id(workspace_id)
                .manifest(self.manifest.clone())
                .fs(self.fs.clone())
                .terminal(self.terminal.clone())
                .backend(self.backend.clone())
                .build();
            State::Next(Box::new(next))
        } else {
            // The user has decided to create a new workspace.
            // Prompt for the workspace name, create a new workspace,
            // and then set the field and continue.
            info!("Let's create a new workspace");
            let workspace_name = self.terminal.prompt_workspace_name();

            // Create the workspace
            let workspace = match self.backend.create_workspace(workspace_name.clone()).await {
                Ok(workspace) => workspace,
                Err(err) => return State::Err(err),
            };

            // Set the workspace name in the manifest
            let mut manifest_guard = self.manifest.lock().await;
            manifest_guard.set_workspace(workspace_name);
            drop(manifest_guard);

            // Continue to the next state
            let next = PromptApplication::builder()
                .manifest(self.manifest.clone())
                .fs(self.fs.clone())
                .terminal(self.terminal.clone())
                .backend(self.backend.clone())
                .workspace_id(workspace.id)
                .build();

            State::Next(Box::new(next))
        }
    }
}

#[derive(Builder)]
struct PromptApplication {
    workspace_id: WorkspaceId,
    manifest: Arc<Mutex<Manifest>>,
    terminal: Arc<Terminal>,
    fs: FileSystem,
    backend: BackendClient,
}

#[async_trait]
impl InitStateMachine for PromptApplication {
    type Output = Arc<Mutex<Manifest>>;

    async fn run(&mut self) -> State<Self::Output> {
        debug_assert!(self.backend.is_authenicated().is_ok());
        // Now that we have an authenticated client and a workspace ID,
        // we can read the applications and ask if they want to
        // use an existing application or create a new one.
        let applications = match self.backend.list_applications(self.workspace_id).await {
            Ok(applications) => applications,
            Err(err) => return State::Err(err),
        };
        let application_names: Vec<_> = applications
            .into_iter()
            .map(|application| application.display_name)
            .collect();
        // We're going to prompt the user to pick out their application
        // from the list, or to create a new one.
        // To give them that option, we have to add a new element
        // to the list.
        let mut options = application_names.clone();
        options.push("+ Create new application".to_owned());
        // Now, we can prompt the user to select an option.
        info!("Which application would you like to use?");
        let selection = self
            .terminal
            .prompt_single_selection("Application", options.as_slice());
        if selection < application_names.len() {
            // The user has selected an existing application.
            let application = application_names[selection].clone();
            let mut manifest_guard = self.manifest.lock().await;
            manifest_guard.set_application(application);
            drop(manifest_guard);
            // Move on to cloud provider selection
            let next = PromptCloudProvider::builder()
                .manifest(self.manifest.clone())
                .fs(self.fs.clone())
                .terminal(self.terminal.clone())
                .backend(self.backend.clone())
                .build();
            State::Next(Box::new(next))
        } else {
            // The user has decided to create a new application.
            // Prompt for the application name and create a new application.
            todo!();
        }
    }
}

#[derive(Builder)]
struct PromptCloudProvider {
    manifest: Arc<Mutex<Manifest>>,
    terminal: Arc<Terminal>,
    fs: FileSystem,
    backend: BackendClient,
}

#[async_trait]
impl InitStateMachine for PromptCloudProvider {
    type Output = Arc<Mutex<Manifest>>;

    async fn run(&mut self) -> State<Self::Output> {
        debug_assert!(self.backend.is_authenicated().is_ok());

        // Prompt the user to select their cloud provider
        let cloud_providers = vec!["AWS".to_owned(), "Cloudflare".to_owned()];
        info!("Which cloud provider are you using?");
        let selection = self
            .terminal
            .prompt_single_selection("Provider", cloud_providers.as_slice());

        let selected_provider = &cloud_providers[selection];

        match selected_provider.as_str() {
            "AWS" => {
                // Move on to AWS setup
                let next = PromptAWSSetup::builder()
                    .manifest(self.manifest.clone())
                    .fs(self.fs.clone())
                    .terminal(self.terminal.clone())
                    .backend(self.backend.clone())
                    .build();
                State::Next(Box::new(next))
            }
            "Cloudflare" => {
                // Move on to Cloudflare setup
                let next = PromptCloudflareSetup::builder()
                    .manifest(self.manifest.clone())
                    .fs(self.fs.clone())
                    .terminal(self.terminal.clone())
                    .backend(self.backend.clone())
                    .build();
                State::Next(Box::new(next))
            }
            _ => return State::Err(miette::miette!("Invalid cloud provider selected")),
        }
    }
}

#[derive(Builder)]
struct PromptCloudflareSetup {
    manifest: Arc<Mutex<Manifest>>,
    terminal: Arc<Terminal>,
    fs: FileSystem,
    backend: BackendClient,
}

impl PromptCloudflareSetup {
    async fn get_worker_name(&self, client: &CloudflareClient) -> Result<String> {
        // Get list of workers from Cloudflare
        let workers = match client.list_workers().await {
            Ok(workers) => workers,
            Err(err) => {
                return Err(miette::miette!(
                    "Failed to fetch workers from Cloudflare: {}",
                    err
                ));
            }
        };

        // Prompt user to select which worker they want to use
        let worker_names: Vec<String> = workers.iter().map(|w| w.id().clone()).collect();
        if worker_names.is_empty() {
            return Err(miette::miette!(
                "No workers found in your Cloudflare account"
            ));
        }

        info!("Which Cloudflare Worker would you like to use?");
        let selection = self
            .terminal
            .prompt_single_selection("Worker", &worker_names);
        Ok(worker_names[selection].clone())
    }

    fn get_artifact_path(&self) -> String {
        loop {
            info!("Please enter the path to your worker's artifacts (e.g., src/):");
            let path_str = self.terminal.prompt_text("Artifact Path");
            let path = Path::new(&path_str);

            // Verify that the path is valid
            if path.exists() && path.is_dir() {
                return path_str;
            } else {
                info!(
                    "The path '{}' is not a valid directory. Please try again.",
                    path_str
                );
            }
        }
    }
}

#[async_trait]
impl InitStateMachine for PromptCloudflareSetup {
    type Output = Arc<Mutex<Manifest>>;

    async fn run(&mut self) -> State<Self::Output> {
        debug_assert!(self.backend.is_authenicated().is_ok());

        // Get account ID from user
        info!("Please enter your Cloudflare account ID:");
        let account_id = self.terminal.prompt_text("Account ID");

        // First, prompt user for API token
        info!("Please enter your Cloudflare API token:");
        let api_token = self.terminal.prompt_text("API Token");

        let client = CloudflareClient::new(account_id.clone(), &api_token);

        let selected_worker_name = match self.get_worker_name(&client).await {
            Ok(name) => name,
            Err(err) => return State::Err(err),
        };

        let artifact_path = self.get_artifact_path();

        // Prompt user for the main module of their worker
        info!("Please enter the main module of your worker (e.g., index.js):");
        let main_module = self.terminal.prompt_text("Main Module");

        // Store all the values in the manifest
        {
            let mut manifest_guard = self.manifest.lock().await;

            let cloudflare_config = CloudflareConfig::new(
                false, // wrangler
                main_module,
                account_id,
                selected_worker_name,
                artifact_path,
            );

            manifest_guard.set_cloudflare_config(cloudflare_config);
        }

        State::Done(self.manifest.clone())
    }
}

#[derive(Builder)]
struct PromptAWSSetup {
    manifest: Arc<Mutex<Manifest>>,
    terminal: Arc<Terminal>,
    fs: FileSystem,
    backend: BackendClient,
}
impl PromptAWSSetup {
    async fn get_lambda_name(
        &self,
        lambda_client: &aws_sdk_lambda::Client,
        region: &str,
    ) -> Result<String> {
        let lambda_functions = match lambda_client.list_functions().send().await {
            Ok(response) => response.functions().to_vec(),
            Err(err) => {
                return Err(miette::miette!(
                    "Failed to list Lambda functions: {:?}",
                    err
                ));
            }
        };

        if lambda_functions.is_empty() {
            return Err(miette::miette!(
                "No Lambda functions found in region {}. Please create a Lambda function first.",
                region
            ));
        }

        let lambda_names: Vec<String> = lambda_functions
            .iter()
            .filter_map(|f| f.function_name().map(|name| name.to_string()))
            .collect();

        info!("Which Lambda function would you like to use?");
        let lambda_selection = self
            .terminal
            .prompt_single_selection("Lambda", &lambda_names);
        Ok(lambda_names[lambda_selection].clone())
    }

    fn prompt_artifact_path(&self) -> String {
        loop {
            info!(
                "Please enter the path to your Lambda deployment artifact (must be a .zip file):"
            );
            let path_str = self.terminal.prompt_text("Artifact Path");
            let path = Path::new(&path_str);

            if !path.exists() {
                warn!("The path '{}' does not exist. Please try again.", path_str);
                continue;
            }

            if !path.is_file() {
                warn!("The path '{}' is not a file. Please try again.", path_str);
                continue;
            }

            if !path_str.to_lowercase().ends_with(".zip") {
                warn!(
                    "The file '{}' is not a .zip file. Please provide a .zip file.",
                    path_str
                );
                continue;
            }

            return path_str;
        }
    }

    async fn get_api_gateway(
        &self,
        apig_client: &aws_sdk_apigateway::Client,
    ) -> Result<(String, String)> {
        info!("Fetching API Gateways...");
        let api_gateways = match apig_client.get_rest_apis().send().await {
            Ok(response) => response.items().to_vec(),
            Err(err) => {
                return Err(miette::miette!("Failed to list API Gateways: {:?}", err));
            }
        };

        if api_gateways.is_empty() {
            return Err(miette::miette!(
                "No API Gateways found. Please create an API Gateway first."
            ));
        }

        let gateway_names: Vec<String> = api_gateways
            .iter()
            .filter_map(|gw| gw.name().map(|name| name.to_string()))
            .collect();

        info!("Which API Gateway would you like to use?");
        let gateway_selection = self
            .terminal
            .prompt_single_selection("API Gateway", &gateway_names);
        let selected_gateway_name = gateway_names[gateway_selection].clone();
        let selected_gateway = &api_gateways[gateway_selection];
        let gateway_id = selected_gateway.id().unwrap().to_string();

        Ok((selected_gateway_name, gateway_id))
    }

    async fn get_stage(
        &self,
        apig_client: &aws_sdk_apigateway::Client,
        gateway_id: &str,
        gateway_name: &str,
    ) -> Result<String> {
        info!("Fetching API Gateway stages...");
        let stages = match apig_client
            .get_stages()
            .rest_api_id(gateway_id)
            .send()
            .await
        {
            Ok(response) => response.item().to_vec(),
            Err(err) => {
                return Err(miette::miette!(
                    "Failed to list API Gateway stages: {:?}",
                    err
                ));
            }
        };

        if stages.is_empty() {
            return Err(miette::miette!(
                "No stages found for API Gateway '{}'. Please create a stage first.",
                gateway_name
            ));
        }

        let stage_names: Vec<String> = stages
            .iter()
            .filter_map(|stage| stage.stage_name().map(|name| name.to_string()))
            .collect();

        info!("Which stage would you like to use?");
        let stage_selection = self.terminal.prompt_single_selection("Stage", &stage_names);
        Ok(stage_names[stage_selection].clone())
    }

    async fn get_resource_method_and_path(
        &self,
        apig_client: &aws_sdk_apigateway::Client,
        gateway_id: &str,
        gateway_name: &str,
    ) -> Result<(String, String)> {
        info!("Fetching API Gateway resources...");
        let resources = match apig_client
            .get_resources()
            .rest_api_id(gateway_id)
            .send()
            .await
        {
            Ok(response) => response.items().to_vec(),
            Err(err) => {
                return Err(miette::miette!(
                    "Failed to list API Gateway resources: {:?}",
                    err
                ));
            }
        };

        if resources.is_empty() {
            return Err(miette::miette!(
                "No resources found for API Gateway '{}'. Please create resources first.",
                gateway_name
            ));
        }

        // Build a list of resource path + method combinations
        let mut resource_options = Vec::new();
        for resource in &resources {
            if let Some(path) = resource.path() {
                if let Some(methods) = resource.resource_methods() {
                    for method in methods.keys() {
                        resource_options.push(format!("{} /{}", method, path));
                    }
                }
            }
        }

        if resource_options.is_empty() {
            return Err(miette::miette!(
                "No resource methods found for API Gateway '{}'. Please configure resource methods first.",
                gateway_name
            ));
        }

        info!("Which resource method and path would you like to use?");
        let resource_selection = self
            .terminal
            .prompt_single_selection("Resource Method", &resource_options);
        let selected_resource_option = &resource_options[resource_selection];

        // Parse the selected option to extract method and path
        let parts: Vec<&str> = selected_resource_option.splitn(2, ' ').collect();
        let selected_method = parts[0].to_string();
        let selected_path = parts[1].to_string();

        Ok((selected_method, selected_path))
    }
}

#[async_trait]
impl InitStateMachine for PromptAWSSetup {
    type Output = Arc<Mutex<Manifest>>;

    async fn run(&mut self) -> State<Self::Output> {
        let aws_config = load_default_aws_config().await;
        let apig_client = aws_sdk_apigateway::Client::new(&aws_config);
        let lambda_client = aws_sdk_lambda::Client::new(aws_config);

        info!("Please enter the AWS region you would like to use (e.g., us-east-2):");
        let region = self.terminal.prompt_text("AWS Region");

        let lambda_name = match self.get_lambda_name(&lambda_client, &region).await {
            Ok(name) => name,
            Err(err) => return State::Err(err),
        };

        let artifact_path = self.prompt_artifact_path();

        let (gateway_name, gateway_id) = match self.get_api_gateway(&apig_client).await {
            Ok(result) => result,
            Err(err) => return State::Err(err),
        };

        let stage_name = match self
            .get_stage(&apig_client, &gateway_id, &gateway_name)
            .await
        {
            Ok(name) => name,
            Err(err) => return State::Err(err),
        };

        let (resource_method, resource_path) = match self
            .get_resource_method_and_path(&apig_client, &gateway_id, &gateway_name)
            .await
        {
            Ok(result) => result,
            Err(err) => return State::Err(err),
        };

        let mut manifest_guard = self.manifest.lock().await;

        // Create and set Lambda config
        let platform_config = PlatformConfig::AwsLambda(AwsLambdaConfig::new(
            lambda_name,
            region.clone(),
            artifact_path,
        ));
        manifest_guard.set_platform_config(platform_config);

        // Create and set APIG config
        let ingress_config = IngressConfig::AwsApiGateway(AwsApiGatewayConfig::new(
            stage_name,
            gateway_name,
            resource_path,
            resource_method,
            region.clone(),
        ));
        manifest_guard.set_ingress_config(ingress_config);

        // Create and set CloudWatch config
        let monitor_config = MonitorConfig::AwsCloudwatch(AwsCloudwatch::default());
        manifest_guard.set_monitor_config(monitor_config);

        State::Done(self.manifest.clone())
    }
}

enum State<T> {
    Done(T),
    Next(Box<dyn InitStateMachine<Output = T>>),
    Err(miette::Report),
}

#[async_trait]
trait InitStateMachine {
    type Output;
    async fn run(&mut self) -> State<Self::Output>;
}

#[cfg(test)]
mod tests {
    use crate::manifest::Manifest;

    use super::InitStateMachine;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(InitStateMachine<Output = Manifest>);
}
