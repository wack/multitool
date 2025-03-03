use async_trait::async_trait;
use multitool_sdk::models::{self, ApplicationConfig, IngressConfig, WebServiceConfig};

use crate::adapters::ingresses::apig::AwsApiGateway;

use super::BoxedIngress;

/// Private trait we use locally to unify the API of the many
/// helper structs.&
/// This is basically the Visitor pattern, walking the config
/// like a tree and incrementing building the data structure
/// as we touch each node.
#[async_trait]
trait Builder {
    async fn build(&self) -> BoxedIngress;
}

pub(crate) struct IngressBuilder {
    config: IngressConfig,
}

impl IngressBuilder {
    pub(crate) fn new(config: IngressConfig) -> Self {
        Self { config }
    }

    pub async fn build(&self) -> BoxedIngress {
        Builder::build(self).await
    }
}

#[async_trait]
impl Builder for IngressBuilder {
    async fn build(&self) -> BoxedIngress {
        todo!("Not implemented since we change the API.");
        // let ApplicationConfig::ApplicationConfigOneOf(appconfig) = self.config.clone();
        // match *appconfig.web_service {
        //     WebServiceConfig::WebServiceConfigOneOf(web_service_config) => {
        //         WebServiceIngressBuilder::new(*web_service_config)
        //             .build()
        //             .await
        //     }
        // }
    }
}

struct WebServiceIngressBuilder {
    // config: models::WebServiceConfigOneOf,
}

#[async_trait]
impl Builder for WebServiceIngressBuilder {
    async fn build(&self) -> BoxedIngress {
        todo!("Not implemented since we changed the API.");
        // match *self.config.aws.ingress {
        //     models::AwsIngressConfig::AwsIngressConfigOneOf(ref aws_ingress) => {
        //         AwsIngressBuilder::new(self.config.aws.region.clone(), *aws_ingress.clone())
        //             .build()
        //             .await
        //     }
        // }
    }
}

struct AwsIngressBuilder {
    // config: AwsIngressConfigOneOf,
    region: String,
}

impl AwsIngressBuilder {
    // fn new(region: String, config: AwsIngressConfigOneOf) -> Self {
    //     Self { config, region }
    // }
}

#[async_trait]
impl Builder for AwsIngressBuilder {
    async fn build(&self) -> BoxedIngress {
        todo!("Not implemented since we changed the API.")
        // let gateway_name = self.config.rest_api_gateway_config.gateway_name.clone();
        // let resource_method = self.config.rest_api_gateway_config.resource_method.clone();
        // let resource_path = self.config.rest_api_gateway_config.resource_path.clone();
        // let stage_name = self.config.rest_api_gateway_config.stage_name.clone();
        // let ingress = AwsApiGateway::builder()
        //     .gateway_name(gateway_name)
        //     .region(self.region.clone())
        //     .resource_path(resource_path)
        //     .resource_method(resource_method)
        //     .stage_name(stage_name)
        //     .build()
        //     .await;
        // Box::new(ingress)
    }
}

impl WebServiceIngressBuilder {
    // fn new(config: models::WebServiceConfigOneOf) -> Self {
    //      Self { config }
    // }
}
#[cfg(test)]
mod tests {
    use crate::adapters::BoxedIngress;
    use miette::{IntoDiagnostic, Result};
    use multitool_sdk::models::IngressConfig;
    use serde_json::{Value, json};

    use super::IngressBuilder;

    fn ingress_json() -> Value {
        json!({
          "rest_api_gateway_config": {
            "gateway_name": "multitool-gateway",
            "stage_name": "dev",
            "resource_path": "/",
            "resource_method": "ANY"
          }
        })
    }

    #[tokio::test]
    #[ignore = "Not implemented since we changed the API."]
    async fn parse_app_config() -> Result<()> {
        // • Get the JSON describing this configuration.
        let config_json = serde_json::to_string(&ingress_json()).into_diagnostic()?;
        // • Marshal it into a type.
        let config_object: IngressConfig = serde_json::from_str(&config_json).into_diagnostic()?;
        // • Try to parse it into a domain type.
        let _: BoxedIngress = IngressBuilder::new(config_object).build().await;
        Ok(())
    }
}
