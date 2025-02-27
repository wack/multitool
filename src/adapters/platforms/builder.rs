use async_trait::async_trait;
use multitool_sdk::models::{self, ApplicationConfig, WebServiceConfig};

use crate::artifacts::LambdaZip;

use super::{BoxedPlatform, LambdaPlatform};

#[async_trait]
pub trait PlatformBuilder {
    async fn build(self) -> BoxedPlatform;
}

struct ApplicationPlatformBuilder {
    config: ApplicationConfig,
    artifact: LambdaZip,
}

impl ApplicationPlatformBuilder {
    fn new(config: ApplicationConfig, artifact: LambdaZip) -> Self {
        Self { config, artifact }
    }
}

#[async_trait]
impl PlatformBuilder for ApplicationPlatformBuilder {
    async fn build(self) -> BoxedPlatform {
        todo!("Not implemented since we changed the API.");
        // let ApplicationConfig::ApplicationConfigOneOf(appconfig) = self.config.clone();
        // match *appconfig.web_service {
        //     WebServiceConfig::WebServiceConfigOneOf(web_service_config) => {
        //         WebServicePlatformBuilder::new(*web_service_config, self.artifact)
        //             .build()
        //             .await
        //     }
        // }
    }
}

struct WebServicePlatformBuilder {
    // config: models::WebServiceConfigOneOf,
    artifact: LambdaZip,
}

#[async_trait]
impl PlatformBuilder for WebServicePlatformBuilder {
    async fn build(self) -> BoxedPlatform {
        todo!("Not implemented since we changed the API.");
        // match *self.config.aws.platform {
        //     models::AwsPlatformConfig::AwsPlatformConfigOneOf(ref aws_platform) => {
        //         AwsPlatformBuilder::new(
        //             self.config.aws.region.clone(),
        //             *aws_platform.clone(),
        //             self.artifact,
        //         )
        //         .build()
        //         .await
        //     }
        // }
    }
}

struct AwsPlatformBuilder {
    // config: AwsPlatformConfigOneOf,
    region: String,
    artifact: LambdaZip,
}

impl AwsPlatformBuilder {
    // fn new(region: String, config: AwsPlatformConfigOneOf, artifact: LambdaZip) -> Self {
    //     Self {
    //         config,
    //         region,
    //         artifact,
    //     }
    // }
}

#[async_trait]
impl PlatformBuilder for AwsPlatformBuilder {
    async fn build(self) -> BoxedPlatform {
        todo!("Not implemented since we changed the API.");
        // let lambda_name = self.config.lambda.name.clone();
        // let platform = LambdaPlatform::builder()
        //     .region(self.region.clone())
        //     .name(lambda_name)
        //     .artifact(self.artifact)
        //     .build()
        //     .await;
        // Box::new(platform)
    }
}

impl WebServicePlatformBuilder {
    // fn new(config: models::WebServiceConfigOneOf, artifact: LambdaZip) -> Self {
    //     Self { config, artifact }
    // }
}

#[cfg(test)]
mod tests {
    use crate::{adapters::BoxedPlatform, artifacts::LambdaZip};
    use miette::{IntoDiagnostic, Result};
    use multitool_sdk::models::ApplicationConfig;
    use serde_json::{Value, json};

    use super::{ApplicationPlatformBuilder, PlatformBuilder};

    fn application_json() -> Value {
        json!({
        "web_service": {
          "aws": {
            "region": "us-east-2",
            "ingress": {
              "rest_api_gateway_config": {
                "gateway_name": "multitool-gateway",
                "stage_name": "dev",
                "resource_path": "/",
                "resource_method": "ANY"
              }
            },
            "platform": {
              "lambda": {
                "name": "multitool-lambda"
              }
            },
            "monitor": "cloudwatch_metrics"
          }
        }})
    }

    #[tokio::test]
    #[ignore = "Not implemented since we changed the API."]
    async fn parse_app_config() -> Result<()> {
        // • Get the JSON describing this configuration.
        let config_json = serde_json::to_string(&application_json()).into_diagnostic()?;
        // • Marshal it into a type.
        let config_object: ApplicationConfig =
            serde_json::from_str(&config_json).into_diagnostic()?;
        // • Try to parse it into a domain type.
        let _: BoxedPlatform = ApplicationPlatformBuilder::new(config_object, LambdaZip::mock())
            .build()
            .await;
        Ok(())
    }
}
