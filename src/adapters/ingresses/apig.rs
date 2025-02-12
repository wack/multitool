use async_trait::async_trait;
use bon::bon;
use miette::Result;

use super::Ingress;

/// AwsApiGateway is the Ingress implementation for AWS API Gateway + Lambda.
/// It's responsible for creating canary deployments on API Gateway, updating their
/// traffic and promoting them, and deploying Lambda functions.
pub struct AwsApiGateway {
    // apig_client: GatewayClient,
    // lambda_client: LambdaClient,
    region: String,
    gateway_name: String,
    stage_name: String,
    resource_path: String,
    resource_method: String,
}

#[bon]
impl AwsApiGateway {
    #[builder]
    pub fn new(
        gateway_name: String,
        stage_name: String,
        resource_path: String,
        resource_method: String,
        region: String,
    ) -> Self {
        Self {
            region,
            gateway_name,
            stage_name,
            resource_path,
            resource_method,
        }
    }
}

#[async_trait]
impl Ingress for AwsApiGateway {
    async fn set_canary_traffic(&mut self, percent: u32) -> Result<()> {
        todo!();
    }
}

/*
// use crate::utils::load_default_aws_config;
// use crate::WholePercent;

use super::Ingress;
use async_trait::async_trait;
// use aws_sdk_apigateway::types::{Op, PatchOperation, RestApi};
use miette::miette;
use miette::{IntoDiagnostic, Result};
use tokio::{fs::File, io::AsyncReadExt};

// use aws_sdk_apigateway::{client::Client as GatewayClient, types::DeploymentCanarySettings};
// use aws_sdk_lambda::{client::Client as LambdaClient, primitives::Blob, types::FunctionCode};



impl AwsApiGateway {
    /// Given a path to the lambda, create a new APIG Ingress.
    pub async fn new(
        artifact_path: PathBuf,
        gateway_name: &str,
        stage_name: &str,
        lambda_name: &str,
    ) -> Result<Self> {
        // let artifact = read_file(artifact_path).await?;
        // Now, configure the AWS SDKs.
        // TODO: Extract Config into a single location so we don't have to
        //       repeat this code every time we initialize an AWS client.
        todo!();
        /*
        let config = load_default_aws_config().await;
        let apig_client = GatewayClient::new(config);
        let lambda_client = LambdaClient::new(config);

        Ok(Self {
            lambda_artifact: artifact,
            apig_client,
            lambda_client,
            gateway_name: gateway_name.to_owned(),
            stage_name: stage_name.to_owned(),
            lambda_name: lambda_name.to_owned(),
        })
        */
    }

    // Helper function to convert an API Gateway's name to its auto-generated AWS ID
    /*
    pub async fn get_api_id_by_name(&self, api_name: &str) -> Result<RestApi> {
        todo!();

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
        */
}
*/
/*
#[async_trait]
impl Ingress for AwsApiGateway {
    async fn deploy(&mut self) -> Result<()> {
        todo!();
        */
/*
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
*/
//    }

/*
    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        todo!();

        let api = self.get_api_id_by_name(&self.gateway_name).await?;
        let api_id = api.id().ok_or(miette!("Couldn't get ID of deployed API"))?;

        let patch_op = PatchOperation::builder()
            .op(Op::Replace)
            .path("/canarySettings/percentTraffic")
            .value(percent.to_string())
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
