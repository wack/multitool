use openapi::models::{self, ApplicationConfig, AwsPlatformConfigOneOf, WebServiceConfig};

use super::{BoxedPlatform, LambdaPlatform};

pub trait PlatformBuilder {
    fn build(&self) -> BoxedPlatform;
}

struct ApplicationPlatformBuilder {
    config: ApplicationConfig,
}

impl ApplicationPlatformBuilder {
    fn new(config: ApplicationConfig) -> Self {
        Self { config }
    }
}

impl PlatformBuilder for ApplicationPlatformBuilder {
    fn build(&self) -> BoxedPlatform {
        let ApplicationConfig::ApplicationConfigOneOf(appconfig) = self.config.clone();
        match *appconfig.web_service {
            WebServiceConfig::WebServiceConfigOneOf(web_service_config) => {
                WebServicePlatformBuilder::new(*web_service_config).build()
            }
        }
    }
}

struct WebServicePlatformBuilder {
    config: models::WebServiceConfigOneOf,
}

impl PlatformBuilder for WebServicePlatformBuilder {
    fn build(&self) -> BoxedPlatform {
        match *self.config.aws.platform {
            models::AwsPlatformConfig::AwsPlatformConfigOneOf(ref aws_platform) => {
                AwsPlatformBuilder::new(self.config.aws.region.clone(), *aws_platform.clone())
                    .build()
            }
        }
    }
}

struct AwsPlatformBuilder {
    config: AwsPlatformConfigOneOf,
    region: String,
}

impl AwsPlatformBuilder {
    fn new(region: String, config: AwsPlatformConfigOneOf) -> Self {
        Self { config, region }
    }
}

impl PlatformBuilder for AwsPlatformBuilder {
    fn build(&self) -> BoxedPlatform {
        let lambda_name = self.config.lambda.name.clone();
        let platform = LambdaPlatform::builder()
            .region(self.region.clone())
            .name(lambda_name)
            .build();
        Box::new(platform)
    }
}

impl WebServicePlatformBuilder {
    fn new(config: models::WebServiceConfigOneOf) -> Self {
        Self { config }
    }
}
#[cfg(test)]
mod tests {
    use crate::adapters::BoxedPlatform;
    use miette::{IntoDiagnostic, Result};
    use openapi::models::ApplicationConfig;
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

    #[test]
    fn parse_app_config() -> Result<()> {
        // • Get the JSON describing this configuration.
        let config_json = serde_json::to_string(&application_json()).into_diagnostic()?;
        // • Marshal it into a type.
        let config_object: ApplicationConfig =
            serde_json::from_str(&config_json).into_diagnostic()?;
        // • Try to parse it into a domain type.
        let _: BoxedPlatform = ApplicationPlatformBuilder::new(config_object).build();
        Ok(())
    }
}
