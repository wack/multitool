use openapi::models::{self, ApplicationConfig, AwsIngressConfigOneOf, WebServiceConfig};

use crate::adapters::{AwsApiGateway, Ingress};

pub trait IngressBuilder {
    fn build(&self) -> Box<dyn Ingress>;
}

struct ApplicationIngressBuilder {
    config: ApplicationConfig,
}

impl ApplicationIngressBuilder {
    fn new(config: ApplicationConfig) -> Self {
        Self { config }
    }
}

impl IngressBuilder for ApplicationIngressBuilder {
    fn build(&self) -> Box<dyn Ingress> {
        let ApplicationConfig::ApplicationConfigOneOf(appconfig) = self.config.clone();
        match *appconfig.web_service {
            WebServiceConfig::WebServiceConfigOneOf(web_service_config) => {
                WebServiceIngressBuilder::new(*web_service_config).build()
            }
        }
    }
}

struct WebServiceIngressBuilder {
    config: models::WebServiceConfigOneOf,
}

impl IngressBuilder for WebServiceIngressBuilder {
    fn build(&self) -> Box<dyn Ingress> {
        match *self.config.aws.ingress {
            models::AwsIngressConfig::AwsIngressConfigOneOf(ref aws_ingress) => {
                AwsIngressBuilder::new(self.config.aws.region.clone(), *aws_ingress.clone()).build()
            }
        }
    }
}

struct AwsIngressBuilder {
    config: AwsIngressConfigOneOf,
    region: String,
}

impl AwsIngressBuilder {
    fn new(region: String, config: AwsIngressConfigOneOf) -> Self {
        Self { config, region }
    }
}

impl IngressBuilder for AwsIngressBuilder {
    fn build(&self) -> Box<dyn Ingress> {
        let gateway_name = self.config.rest_api_gateway_config.gateway_name.clone();
        let resource_method = self.config.rest_api_gateway_config.resource_method.clone();
        let resource_path = self.config.rest_api_gateway_config.resource_path.clone();
        let stage_name = self.config.rest_api_gateway_config.stage_name.clone();
        let ingress = AwsApiGateway::builder()
            .gateway_name(gateway_name)
            .region(self.region.clone())
            .resource_path(resource_path)
            .resource_method(resource_method)
            .stage_name(stage_name)
            .build();
        Box::new(ingress)
    }
}

impl WebServiceIngressBuilder {
    fn new(config: models::WebServiceConfigOneOf) -> Self {
        Self { config }
    }
}
#[cfg(test)]
mod tests {
    use crate::adapters::BoxIngress;
    use miette::{IntoDiagnostic, Result};
    use openapi::models::ApplicationConfig;
    use serde_json::{json, Value};

    use super::{ApplicationIngressBuilder, IngressBuilder};

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
        let _: BoxIngress = ApplicationIngressBuilder::new(config_object).build();
        Ok(())
    }
}
