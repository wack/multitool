use async_trait::async_trait;
use bon::bon;
use miette::{IntoDiagnostic as _, Result, miette};

use crate::{
    Shutdownable, WholePercent, subsystems::ShutdownResult, utils::load_default_aws_config,
};

use aws_sdk_apigateway::{
    client::Client as GatewayClient,
    types::{Op, PatchOperation, RestApi},
};

use super::Ingress;

/// AwsApiGateway is the Ingress implementation for AWS API Gateway + Lambda.
/// It's responsible for creating canary deployments on API Gateway, updating their
/// traffic and promoting them, and deploying Lambda functions.
pub struct AwsApiGateway {
    apig_client: GatewayClient,
    region: String,
    gateway_name: String,
    stage_name: String,
    resource_path: String,
    resource_method: String,
}

#[bon]
impl AwsApiGateway {
    #[builder]
    pub async fn new(
        gateway_name: String,
        stage_name: String,
        resource_path: String,
        resource_method: String,
        region: String,
    ) -> Self {
        let config = load_default_aws_config().await;
        let apig_client = GatewayClient::new(config);
        Self {
            apig_client,
            region,
            gateway_name,
            stage_name,
            resource_path,
            resource_method,
        }
    }

    // Helper function to convert an API Gateway's name to its auto-generated AWS ID
    pub async fn get_api_id_by_name(&self, api_name: &str) -> Result<RestApi> {
        let all_apis = self
            .apig_client
            .get_rest_apis()
            .send()
            .await
            .into_diagnostic()?;

        let api = all_apis
            .items()
            .iter()
            .find(|api| api.name().unwrap() == api_name)
            .ok_or(miette!(
                "Could not find an API Gateway with the name: {}",
                api_name
            ))?;

        Ok(api.clone())
    }
}

#[async_trait]
impl Ingress for AwsApiGateway {
    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        let api = self.get_api_id_by_name(&self.gateway_name).await?;
        let api_id = api.id().ok_or(miette!("Couldn't get ID of deployed API"))?;
        // Remove the trailing percent sign from the string.
        let percent_string = percent.to_string();
        let percent_trimmed = percent_string.trim_end_matches('%');

        let patch_op = PatchOperation::builder()
            .op(Op::Replace)
            .path("/canarySettings/percentTraffic")
            .value(percent_trimmed)
            .build();

        self.apig_client
            .update_stage()
            .rest_api_id(api_id)
            .stage_name(&self.stage_name)
            .patch_operations(patch_op)
            .send()
            .await
            .into_diagnostic()?;

        Ok(())
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        todo!("Not yet implemented.")
    }

    async fn promote_canary(&mut self) -> Result<()> {
        todo!("Not yet implemented.")
    }
}

#[async_trait]
impl Shutdownable for AwsApiGateway {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!("What should the APIG do abnormal shutdown?");
    }
}
