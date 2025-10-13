use std::path::{Path, PathBuf};

use clap::Args;

use crate::{MULTITOOL_ORIGIN, manifest::Manifest};

#[derive(Args, Clone)]
pub struct RunSubcommand {
    #[arg(short, long, env = "MULTI_WORKSPACE")]
    workspace: Option<String>,
    #[arg(short, long, env = "MULTI_APPLICATION")]
    application: Option<String>,
    #[arg(long, short = 'o', default_value = Some(MULTITOOL_ORIGIN), env = "MULTI_ORIGIN")]
    origin: Option<String>,

    ///Cloudflare config
    /// The Cloudflare account ID to use when deploying to Workers.
    #[arg(long, env = "CLOUDFLARE_ACCOUNT_ID")]
    cloudflare_account_id: Option<String>,
    /// The API token to use for Cloudflare API requests.
    #[arg(long, env = "CLOUDFLARE_API_TOKEN")]
    cloudflare_api_token: Option<String>,
    /// The path to the Cloudflare Worker project directory.
    #[arg(long, env = "CLOUDFLARE_PROJECT_DIR")]
    cloudflare_project_dir: Option<PathBuf>,

    /// AWS Config
    /// The AWS region to deploy into.
    #[arg(long, env = "AWS_REGION")]
    aws_region: Option<String>,
    /// The AWS API Gateway's Name
    #[arg(long, env = "AWS_GATEWAY_NAME")]
    aws_gateway_name: Option<String>,
    /// The AWS API Gateway Stage's Name
    #[arg(long, env = "AWS_STAGE_NAME")]
    aws_stage_name: Option<String>,
    /// The AWS API Gateway Stage's Path (with leading slash)
    #[arg(long, env = "AWS_RESOURCE_PATH")]
    aws_resource_path: Option<String>,
    /// The AWS API Gateway Stage's HTTP Method
    #[arg(long, env = "AWS_RESOURCE_METHOD")]
    aws_resource_method: Option<String>,
    /// The AWS Lambda's Name
    #[arg(long, env = "AWS_LAMBDA_NAME")]
    aws_lambda_name: Option<String>,
    /// The path to the artifact to upload to AWS.
    #[arg(long, env = "AWS_ARTIFACT_PATH")]
    aws_artifact_path: Option<PathBuf>,
}

impl RunSubcommand {
    pub fn cloudflare_account_id(&self) -> Option<&str> {
        self.cloudflare_account_id.as_deref()
    }

    pub fn cloudflare_api_token(&self) -> Option<&str> {
        self.cloudflare_api_token.as_deref()
    }

    pub fn cloudflare_project_dir(&self) -> Option<&Path> {
        self.cloudflare_project_dir.as_deref()
    }

    pub fn aws_region(&self) -> Option<&str> {
        self.aws_region.as_deref()
    }

    pub fn aws_gateway_name(&self) -> Option<&str> {
        self.aws_gateway_name.as_deref()
    }

    pub fn aws_stage_name(&self) -> Option<&str> {
        self.aws_stage_name.as_deref()
    }

    pub fn aws_resource_path(&self) -> Option<&str> {
        self.aws_resource_path.as_deref()
    }

    pub fn aws_resource_method(&self) -> Option<&str> {
        self.aws_resource_method.as_deref()
    }

    pub fn aws_lambda_name(&self) -> Option<&str> {
        self.aws_lambda_name.as_deref()
    }

    pub fn aws_artifact_path(&self) -> Option<&Path> {
        self.aws_artifact_path.as_deref()
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

    /// Merge the values from this manifest file into this struct.
    pub fn coalesce(&mut self, manifest: &Manifest) {
        let workspace_fallback = manifest.workspace().map(|elem| elem.to_owned());
        let application_fallback = manifest.application().map(|elem| elem.to_owned());
        self.workspace = self.workspace.clone().or(workspace_fallback);
        self.application = self.application.clone().or(application_fallback);
    }
}
