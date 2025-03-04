use async_trait::async_trait;
use multitool_sdk::models::{IngressConfig, IngressConfigOneOfAwsRestApiGateway};

use crate::adapters::ingresses::apig::AwsApiGateway;

use super::BoxedIngress;

/// Private trait we use locally to unify the API of the many
/// helper structs.&
/// This is basically the Visitor pattern, walking the config
/// like a tree and incrementing building the data structure
/// as we touch each node.
#[async_trait]
trait Builder {
    async fn build(self) -> BoxedIngress;
}

pub(crate) struct IngressBuilder {
    config: IngressConfig,
}

impl IngressBuilder {
    pub(crate) fn new(config: IngressConfig) -> Self {
        Self { config }
    }

    pub async fn build(self) -> BoxedIngress {
        Builder::build(self).await
    }
}

#[async_trait]
impl Builder for IngressBuilder {
    async fn build(self) -> BoxedIngress {
        match self.config {
            IngressConfig::IngressConfigOneOf(ingress_conf) => {
                AwsGatewayIngressBuilder::new(*ingress_conf.aws_rest_api_gateway)
                    .build()
                    .await
            }
        }
    }
}

struct AwsGatewayIngressBuilder {
    conf: IngressConfigOneOfAwsRestApiGateway,
}

impl AwsGatewayIngressBuilder {
    fn new(conf: IngressConfigOneOfAwsRestApiGateway) -> Self {
        Self { conf }
    }
}

#[async_trait]
impl Builder for AwsGatewayIngressBuilder {
    async fn build(self) -> BoxedIngress {
        let ingress = AwsApiGateway::builder()
            .gateway_name(self.conf.gateway_name)
            .region(self.conf.region)
            .stage_name(self.conf.stage_name)
            .resource_path(self.conf.resource_path)
            .resource_method(self.conf.resource_method)
            .build()
            .await;
        Box::new(ingress)
    }
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
          "aws_rest_api_gateway": {
            "gateway_name": "multitool-gateway",
            "region": "us-east-2",
            "stage_name": "dev",
            "resource_path": "/",
            "resource_method": "ANY"
          }
        })
    }

    #[tokio::test]
    async fn parse_ingress_config() -> Result<()> {
        // • Get the JSON describing this configuration.
        let config_json = serde_json::to_string(&ingress_json()).into_diagnostic()?;
        // • Marshal it into a type.
        let config_object: IngressConfig = serde_json::from_str(&config_json).into_diagnostic()?;
        // • Try to parse it into a domain type.
        let _: BoxedIngress = IngressBuilder::new(config_object).build().await;
        Ok(())
    }
}
