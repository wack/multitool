use async_trait::async_trait;
use bon::bon;
use miette::{miette, IntoDiagnostic as _, Result};

use crate::{
    subsystems::ShutdownResult, utils::load_default_aws_config, Shutdownable, WholePercent,
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
        todo!()
    }

    async fn promote_canary(&mut self) -> Result<()> {
        todo!()
    }
}

#[async_trait]
impl Shutdownable for AwsApiGateway {
    async fn shutdown(&mut self) -> ShutdownResult {
        todo!();
    }
}

/*
#[async_trait]
impl Ingress for AwsApiGateway {
    async fn deploy(&mut self) -> Result<()> {
        todo!();
// First, we need to deploy the new version of the lambda

// Parse the bytes into the format AWS wants
let code = Blob::from(self.lambda_artifact.clone());

// Turn it into an uploadable zip file
let function_code = FunctionCode::builder().zip_file(code).build();
let zip_file = function_code
    .zip_file()
    .ok_or(miette!("Couldn't zip lambda code"))?;

// Upload it to Lambda
let res = self
    .lambda_client
    .update_function_code()
    .function_name(&self.lambda_name)
    .zip_file(zip_file.clone())
    .send()
    .await
    .into_diagnostic()?;

let lambda_arn = res
    .function_arn()
    .ok_or(miette!("Couldn't get ARN of deployed lambda"))?;

let version = res
    .version()
    .ok_or(miette!("Couldn't get version of deployed lambda"))?;

let api = self.get_api_id_by_name(&self.gateway_name).await?;
let api_id = api.id().ok_or(miette!("Couldn't get ID of deployed API"))?;

// Next, we need to create a new deployment, pointing at our
// new lambda version with canary settings
self.apig_client
    .put_integration()
    .rest_api_id(api_id)
    .uri(format!("{}:{}", lambda_arn, version))
    .send()
    .await
    .into_diagnostic()?;

// Create a deployment with canary settings to deploy our new lambda
self.apig_client
    .create_deployment()
    .rest_api_id(api_id)
    .stage_name(&self.stage_name)
    .canary_settings(
        DeploymentCanarySettings::builder()
            // This is set to 0 explicitly here since the first step of the pipeline
            // is to increase traffic
            .percent_traffic(0.0)
            .build(),
    )
    .send()
    .await
    .into_diagnostic()?;

Ok(())
//    }
    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        todo!();
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        todo!();
        let api = self.get_api_id_by_name(&self.gateway_name).await?;
        let api_id = api.id().ok_or(miette!("Couldn't get ID of deployed API"))?;

        // Updates the stage to delete any canary settings from the API Gateway
        let patch_op = PatchOperation::builder()
            .op(Op::Remove)
            .path("/canarySettings")
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

    async fn promote_canary(&mut self) -> Result<()> {
        todo!();
        let api = self.get_api_id_by_name(&self.gateway_name).await?;
        let api_id = api.id().ok_or(miette!("Couldn't get ID of deployed API"))?;

        // Overwrite the main deployment's ID with the canary's
        let replace_deployment_op = PatchOperation::builder()
            .op(Op::Copy)
            .from("/canarySettings/deploymentId")
            .path("/deploymentId")
            .build();

        // Deletes all canary settings from the API Gateway so we're ready for the next
        // canary deployment
        let delete_canary_op = PatchOperation::builder()
            .op(Op::Remove)
            .path("/canarySettings")
            .build();

        // Send request to update stage
        self.apig_client
            .update_stage()
            .rest_api_id(api_id)
            .stage_name(&self.stage_name)
            .patch_operations(replace_deployment_op)
            .patch_operations(delete_canary_op)
            .send()
            .await
            .into_diagnostic()?;

        Ok(())

    }
}
        */
