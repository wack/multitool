use async_trait::async_trait;
use miette::{bail, IntoDiagnostic, Result};
use openapi::apis::applications_api::{get_application, list_applications};
use openapi::apis::users_api::login;
use openapi::apis::workspaces_api::list_workspaces;
use openapi::models::{
    ApplicationConfig as AppConf, AwsPlatformConfig, AwsPlatformConfigOneOfLambda,
};
use openapi::models::{ApplicationDetails, LoginRequest, WebServiceConfig, WorkspaceSummary};
use uuid::Uuid;

use crate::fs::UserCreds;
use crate::Cli;

use super::config::{AwsRestApiGatewayConfig, IngressConfig, LambdaConfig, PlatformConfig};
use super::{ApplicationConfig, BackendClient, BackendConfig, Session};

pub struct MultiToolBackend {
    conf: BackendConfig,
}

#[async_trait]
impl BackendClient for MultiToolBackend {
    async fn fetch_config(&self, workspace: &str, application: &str) -> Result<ApplicationConfig> {
        // • First, we have to exchange the workspace name for it's id.
        let workspace = self.get_workspace_by_name(workspace).await?;
        // • Then, we can do the same with the application name.
        let _application = self
            .get_application_by_name(workspace.id, application)
            .await?;
        todo!()
    }

    /// Exchange auth credentials with the server for an auth token.
    /// Account is either the user's account name or email address.
    async fn exchange_creds(&self, email: &str, password: &str) -> Result<Session> {
        // • Create and send the request, marshalling the result
        //   into user credentials.
        let req = LoginRequest {
            email: email.to_owned(),
            password: password.to_owned(),
        };
        let creds: UserCreds = login(&self.conf, req).await.into_diagnostic()?.into();

        Ok(Session::User(creds))
    }
}

impl MultiToolBackend {
    /// Return a new backend client for the MultiTool backend.
    pub fn new(cli: &Cli) -> Self {
        let conf = BackendConfig::from(cli);
        Self { conf }
    }

    /// Return information about the workspace given its name.
    async fn get_workspace_by_name(&self, name: &str) -> Result<WorkspaceSummary> {
        let mut workspaces = list_workspaces(&self.conf, Some(name))
            .await
            .into_diagnostic()?
            .workspaces;

        if workspaces.len() > 1 {
            bail!("More than one workspace with the given name found.");
        } else if workspaces.len() < 1 {
            bail!("No workspace with the given name exists for this account");
        } else {
            // TODO: We can simplify this code with .ok_or()
            Ok(workspaces.pop().unwrap())
        }
    }

    // This soup is because of the nastiness of OpenAPI Generator.
    fn marshall_config(app_conf: AppConf) -> ApplicationConfig {
        todo!()
        /*
        let AppConf::ApplicationConfigOneOf(conf_one_of) = app_conf;
        let WebServiceConfig::WebServiceConfigOneOf(web_service) = *conf_one_of.web_service;
        let aws = *web_service.aws;
        let ingress = *aws.ingress;
        let monitor = aws.monitor;
        let AwsPlatformConfig::AwsPlatformConfigOneOf(platform) = *aws.platform;
        let lambda_name = platform.lambda.name;
        let region = aws.region;
        ApplicationConfig {
            platform: PlatformConfig::Lambda(LambdaConfig {
                region: region.clone(),
                name: lambda_name,
            }),
            ingress: IngressConfig::RestApiGateway(AwsRestApiGatewayConfig{
                region: region.clone(),
                gateway_name: todo!(),
                stage_name: todo!(),
                resource_path: todo!(),
                resource_method: todo!(),
            })

            monitor: (),
        }
        */
    }

    /// Given the id of the workspace containing the application, and the application's
    /// name, fetch the application's information.
    async fn get_application_by_name(
        &self,
        workspace_id: Uuid,
        name: &str,
    ) -> Result<ApplicationDetails> {
        let mut applications: Vec<_> =
            list_applications(&self.conf, workspace_id.to_string().as_ref())
                .await
                .into_diagnostic()?
                .applications
                .into_iter()
                .filter(|elem| elem.display_name == name)
                .collect();

        let application = if applications.len() > 1 {
            bail!("More than one application with the given name found.");
        } else if applications.len() < 1 {
            bail!("No application with the given name exists for this account");
        } else {
            // TODO: We can simplify this code with .ok_or()
            applications.pop().unwrap()
        };

        get_application(
            &self.conf,
            workspace_id.to_string().as_ref(),
            application.id.to_string().as_ref(),
        )
        .await
        .into_diagnostic()
    }
}
