use async_trait::async_trait;
use bon::bon;
use miette::{Result, miette};
use tracing::info;

use crate::{
    Shutdownable, artifacts::LambdaZip, subsystems::ShutdownResult, utils::load_default_aws_config,
};
use aws_sdk_lambda::{client::Client, error::SdkError, primitives::Blob, types::FunctionCode};

use super::Platform;

pub struct LambdaPlatform {
    client: Client,
    region: String,
    name: String,
    artifact: LambdaZip,
    arn: Option<String>,
}

#[bon]
impl LambdaPlatform {
    #[builder]
    pub async fn new(region: String, name: String, artifact: LambdaZip) -> Self {
        let config = load_default_aws_config().await;
        let client = aws_sdk_lambda::Client::new(config);
        Self {
            client,
            region,
            name,
            artifact,
            arn: None,
        }
    }
}

#[async_trait]
impl Platform for LambdaPlatform {
    /// Update the Lambda code with the zip we're holding.
    async fn deploy(&mut self) -> Result<String> {
        info!("Deploying Lambda!");
        // First, we need to deploy the new version of the lambda
        // Parse the bytes into the format AWS wants
        let code = Blob::from(self.artifact.as_ref());

        // Turn it into an uploadable zip file
        let function_code = FunctionCode::builder().zip_file(code).build();
        let zip_file = function_code
            .zip_file()
            .ok_or(miette!("Couldn't zip lambda code"))?;

        // Upload it to Lambda
        let res = self
            .client
            .update_function_code()
            .publish(true)
            .function_name(&self.name)
            .zip_file(zip_file.clone())
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
                miette!("Failed to deploy Lambda: {}", error_message)
            })?;

        let function_arn = res
            .function_arn()
            .map(ToString::to_string)
            .ok_or(miette!("Couldn't get ARN of deployed lambda"))?;

        self.arn = Some(function_arn);
        self.arn
            .clone()
            .ok_or_else(|| miette!("No ARN returned from AWS"))
    }

    // There's nothing to yank when the platform is a lambda
    async fn yank_canary(&mut self) -> Result<()> {
        Ok(())
    }

    async fn delete_canary(&mut self) -> Result<()> {
        self.client
            .delete_function()
            .function_name(self.arn.as_ref().ok_or(miette!("Lambda ARN not set"))?)
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
                miette!("Failed to delete Lambda: {}", error_message)
            })?;

        Ok(())
    }

    async fn promote_rollout(&mut self) -> Result<()> {
        todo!("I don't think Lambdas promote until we support Lambda Aliases.")
    }
}

#[async_trait]
impl Shutdownable for LambdaPlatform {
    async fn shutdown(&mut self) -> ShutdownResult {
        // When we get the shutdown signal, we don't want to do anything to let
        // users debug their lamdbda
        Ok(())
    }
}
