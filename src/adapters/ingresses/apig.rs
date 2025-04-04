use async_trait::async_trait;
use bon::bon;
use miette::{Result, miette};
use tracing::{debug, info};

use crate::{
    Shutdownable, WholePercent, subsystems::ShutdownResult, utils::load_default_aws_config,
};

use aws_sdk_apigateway::{
    client::Client as GatewayClient,
    error::SdkError,
    types::{DeploymentCanarySettings, Op, PatchOperation, Resource, RestApi},
};
use aws_sdk_lambda::client::Client as LambdaClient;

use super::Ingress;

/// AwsApiGateway is the Ingress implementation for AWS API Gateway + Lambda.
/// It's responsible for creating canary rollouts on API Gateway, updating their
/// traffic and promoting them, and deploying Lambda functions.
pub struct AwsApiGateway {
    apig_client: GatewayClient,
    lambda_client: LambdaClient,
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
        // TODO: when we add more platforms, we'll need to move this into the lambda
        let lambda_client = LambdaClient::new(config);

        Self {
            apig_client,
            lambda_client,
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
            .map_err(|err| {
                let error_message = match err {
                    SdkError::ServiceError(service_err) => {
                        // Extract the specific service error details
                        format!(
                            "{}",
                            service_err
                                .err()
                                .meta()
                                .message()
                                .unwrap_or("No error message found")
                        )
                    }
                    _ => format!("{:?}", err),
                };
                miette!("Failed to get API Gateway: {}", error_message)
            })?;

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

    /// Helper function to convert an API Gateway Resource's name to its auto-generated AWS ID
    pub async fn get_resource_id_by_path(
        &self,
        api_id: &str,
        resource_name: &str,
    ) -> Result<Resource> {
        let all_resources = self
            .apig_client
            .get_resources()
            .rest_api_id(api_id)
            .send()
            .await
            .map_err(|err| {
                let error_message = match err {
                    SdkError::ServiceError(service_err) => {
                        // Extract the specific service error details
                        format!(
                            "{}",
                            service_err
                                .err()
                                .meta()
                                .message()
                                .unwrap_or("No error message found")
                        )
                    }
                    _ => format!("{:?}", err),
                };
                miette!("Failed to get API Gateway Resource: {}", error_message)
            })?;

        let resource = all_resources
            .items()
            .iter()
            .find(|resource| resource.path().unwrap() == resource_name)
            .ok_or(miette!(
                "Could not find an API Gateway Resource with the name: {}",
                resource_name
            ))?;

        Ok(resource.clone())
    }

    async fn remove_canary_settings(&mut self) -> Result<()> {
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
            .map_err(|err| {
                let error_message = match err {
                    SdkError::ServiceError(service_err) => {
                        // Extract the specific service error details
                        format!(
                            "{}",
                            service_err
                                .err()
                                .meta()
                                .message()
                                .unwrap_or("No error message found")
                        )
                    }
                    _ => format!("{:?}", err),
                };
                miette!(
                    "Failed to remove canary settings from API Gateway: {}",
                    error_message
                )
            })?;

        Ok(())
    }
}

#[async_trait]
impl Ingress for AwsApiGateway {
    async fn release_canary(&mut self, platform_id: String) -> Result<()> {
        debug!("Releasing canary in API Gateway!");
        // Get the auto-generated API ID and Resource ID
        let api = self.get_api_id_by_name(&self.gateway_name).await?;
        let api_id = api.id().ok_or(miette!("Couldn't get ID of API Gateway"))?;

        let resource = self
            .get_resource_id_by_path(api_id, &self.resource_path)
            .await?;
        let resource_id = resource
            .id()
            .ok_or(miette!("Couldn't get ID of API Gateway Resource"))?;

        // Ensure we add invoke permissions to the new version of the lambda
        // NOTE: All calls to invoke the function will fail unless this is explicitly added
        self.lambda_client
            .add_permission()
            .function_name(platform_id.clone())
            .statement_id(format!("apigateway-permission-{}", api_id))
            .action("lambda:InvokeFunction")
            .principal("apigateway.amazonaws.com")
            .send()
            .await
            .map_err(|err| {
                let error_message = match err {
                    SdkError::ServiceError(service_err) => {
                        // Extract the specific service error details
                        format!(
                            "{}",
                            service_err
                                .err()
                                .meta()
                                .message()
                                .unwrap_or("No error message found")
                        )
                    }
                    _ => format!("{:?}", err),
                };
                miette!(
                    "Failed to add invoke permission to Lambda: {}",
                    error_message
                )
            })?;

        // Update our API Gateway to point at our new lambda version
        let patch_op = PatchOperation::builder()
            .op(Op::Replace)
            .path("/uri")
            .value(format!(
                "arn:aws:apigateway:{}:lambda:path/2015-03-31/functions/{}/invocations",
                self.region, platform_id
            ))
            .build();

        self.apig_client
            .update_integration()
            .rest_api_id(api_id)
            .resource_id(resource_id)
            .http_method(&self.resource_method)
            .patch_operations(patch_op)
            .send()
            .await
            .map_err(|err| {
                let error_message = match err {
                    SdkError::ServiceError(service_err) => {
                        // Extract the specific service error details
                        format!(
                            "{}",
                            service_err
                                .err()
                                .meta()
                                .message()
                                .unwrap_or("No error message found")
                        )
                    }
                    _ => format!("{:?}", err),
                };
                miette!(
                    "Failed to update API Gateway integration request: {}",
                    error_message
                )
            })?;

        // Create a rollout with canary settings to deploy our new lambda
        self.apig_client
            .create_deployment()
            .rest_api_id(api_id)
            .stage_name(&self.stage_name)
            .canary_settings(
                DeploymentCanarySettings::builder()
                    // This is set to 0 explicitly here since the first step of the pipeline
                    // is to collecty baseline traffic
                    .percent_traffic(0.0)
                    .build(),
            )
            .send()
            .await
            .map_err(|err| {
                let error_message = match err {
                    SdkError::ServiceError(service_err) => {
                        // Extract the specific service error details
                        format!(
                            "{}",
                            service_err
                                .err()
                                .meta()
                                .message()
                                .unwrap_or("No error message found")
                        )
                    }
                    _ => format!("{:?}", err),
                };
                miette!(
                    "Failed to update API Gateway canary settings: {}",
                    error_message
                )
            })?;

        Ok(())
    }

    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()> {
        info!("Setting API Gateway canary traffic to {percent}.");
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
            .map_err(|err| {
                let error_message = match err {
                    SdkError::ServiceError(service_err) => {
                        // Extract the specific service error details
                        format!(
                            "{}",
                            service_err
                                .err()
                                .meta()
                                .message()
                                .unwrap_or("No error message found")
                        )
                    }
                    _ => format!("{:?}", err),
                };
                miette!(
                    "Failed to update API Gateway canary traffic: {}",
                    error_message
                )
            })?;

        Ok(())
    }

    async fn rollback_canary(&mut self) -> Result<()> {
        info!("Rolling back canary rollout in API Gateway.");
        self.remove_canary_settings().await
    }

    async fn promote_canary(&mut self) -> Result<()> {
        info!("Promoting canary rollout in API Gateway!");
        let api = self.get_api_id_by_name(&self.gateway_name).await?;
        let api_id = api.id().ok_or(miette!("Couldn't get ID of deployed API"))?;

        // Overwrite the main rollout's ID with the canary's
        let replace_rollout_op = PatchOperation::builder()
            .op(Op::Copy)
            .from("/canarySettings/rolloutId")
            .path("/rolloutId")
            .build();

        // Deletes all canary settings from the API Gateway so we're ready for the next
        // canary rollout
        let delete_canary_op = PatchOperation::builder()
            .op(Op::Remove)
            .path("/canarySettings")
            .build();

        // Send request to update stage
        self.apig_client
            .update_stage()
            .rest_api_id(api_id)
            .stage_name(&self.stage_name)
            .patch_operations(replace_rollout_op)
            .patch_operations(delete_canary_op)
            .send()
            .await
            .map_err(|err| {
                let error_message = match err {
                    SdkError::ServiceError(service_err) => {
                        // Extract the specific service error details
                        format!(
                            "{}",
                            service_err
                                .err()
                                .meta()
                                .message()
                                .unwrap_or("No error message found")
                        )
                    }
                    _ => format!("{:?}", err),
                };
                miette!("Failed to promote canary in API Gateway: {}", error_message)
            })?;

        Ok(())
    }
}

#[async_trait]
impl Shutdownable for AwsApiGateway {
    async fn shutdown(&mut self) -> ShutdownResult {
        // When we get the shutdown signal, we should delete any Canary settings we've set
        self.remove_canary_settings().await?;
        Ok(())
    }
}
