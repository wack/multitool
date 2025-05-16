use std::path::{Path, PathBuf};

use clap::Args;
use derive_getters::Getters;

use crate::{MULTITOOL_ORIGIN, manifest::Manifest};

#[derive(Args, Clone)]
pub struct RunSubcommand {
    // #[arg(short, long, env = "MULTI_WORKSPACE", required = false, default_value = project_manifest().workspace())]
    #[arg(short, long, env = "MULTI_WORKSPACE")]
    workspace: Option<String>,
    // #[arg(short, long, env = "MULTI_APPLICATION", required = false, default_value = project_manifest().application())]
    #[arg(short, long, env = "MULTI_APPLICATION")]
    application: Option<String>,
    /// The path to the zipped serverless function.
    #[arg(value_name = "FILE")]
    artifact_path: PathBuf,

    #[arg(long, short = 'o', default_value = Some(MULTITOOL_ORIGIN))]
    origin: Option<String>,

    /// The Cloudflare account ID to use when deploying to Workers.
    #[arg(long, env = "CLOUDFLARE_ACCOUNT_ID")]
    cloudflare_account_id: Option<String>,
    /// The name of the Cloudflare Worker to deploy.
    #[arg(long, env = "CLOUDFLARE_WORKER_NAME")]
    cloudflare_worker_name: Option<String>,
    /// The name of the Cloudflare Worker to deploy.
    #[arg(long, env = "CLOUDFLARE_API_TOKEN")]
    cloudflare_api_token: Option<String>,
}

impl RunSubcommand {
    pub fn cloudflare_account_id(&self) -> Option<&str> {
        self.cloudflare_account_id.as_deref()
    }

    pub fn cloudflare_worker_name(&self) -> Option<&str> {
        self.cloudflare_worker_name.as_deref()
    }

    pub fn cloudflare_api_token(&self) -> Option<&str> {
        self.cloudflare_api_token.as_deref()
    }

    pub fn workspace(&self) -> Option<&str> {
        self.workspace.as_deref()
    }

    pub fn application(&self) -> Option<&str> {
        self.application.as_deref()
    }

    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    pub fn artifact_path(&self) -> impl AsRef<Path> {
        &self.artifact_path
    }

    /// Merge the values from this manifest file into this struct.
    pub fn coalesce(&mut self, manifest: &Manifest) {
        let workspace_fallback = manifest.workspace().map(|elem| elem.to_owned());
        let application_fallback = manifest.application().map(|elem| elem.to_owned());
        self.workspace = self.workspace.clone().or(workspace_fallback);
        self.application = self.application.clone().or(application_fallback);
    }
}
