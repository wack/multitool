use async_trait::async_trait;
use multitool_sdk::models::{self, ApplicationConfig, PlatformConfig, WebServiceConfig};

use crate::artifacts::LambdaZip;

use super::BoxedPlatform;

#[async_trait]
trait Builder {
    async fn build(self) -> BoxedPlatform;
}

pub(crate) struct PlatformBuilder {
    config: PlatformConfig,
    artifact: LambdaZip,
}

impl PlatformBuilder {
    pub(crate) fn new(config: PlatformConfig, artifact: LambdaZip) -> Self {
        Self { config, artifact }
    }

    pub async fn build(self) -> BoxedPlatform {
        Builder::build(self).await
    }
}

#[async_trait]
impl Builder for PlatformBuilder {
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
impl Builder for WebServicePlatformBuilder {
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
impl Builder for AwsPlatformBuilder {
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
    use multitool_sdk::models::PlatformConfig;
    use serde_json::{Value, json};

    use super::PlatformBuilder;

    fn platform_json() -> Value {
        json!({
              "lambda": {
                "name": "multitool-lambda"
              }
            }
        )
    }

    #[tokio::test]
    #[ignore = "Not implemented since we changed the API."]
    async fn parse_app_config() -> Result<()> {
        // • Get the JSON describing this configuration.
        let config_json = serde_json::to_string(&platform_json()).into_diagnostic()?;
        // • Marshal it into a type.
        let config_object: PlatformConfig = serde_json::from_str(&config_json).into_diagnostic()?;
        // • Try to parse it into a domain type.
        let _: BoxedPlatform = PlatformBuilder::new(config_object, LambdaZip::mock())
            .build()
            .await;
        Ok(())
    }
}
