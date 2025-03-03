use async_trait::async_trait;
use multitool_sdk::models::{PlatformConfig, PlatformConfigOneOfAwsLambda};

use crate::artifacts::LambdaZip;

use super::{BoxedPlatform, lambda::LambdaPlatform};

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
        match self.config {
            PlatformConfig::PlatformConfigOneOf(platform_conf) => {
                AwsLambdaPlatformBuilder::new(*platform_conf.aws_lambda, self.artifact)
                    .build()
                    .await
            }
        }
    }
}

struct AwsLambdaPlatformBuilder {
    config: PlatformConfigOneOfAwsLambda,
    artifact: LambdaZip,
}

impl AwsLambdaPlatformBuilder {
    fn new(config: PlatformConfigOneOfAwsLambda, artifact: LambdaZip) -> Self {
        Self { config, artifact }
    }
}

#[async_trait]
impl Builder for AwsLambdaPlatformBuilder {
    async fn build(self) -> BoxedPlatform {
        let lambda = LambdaPlatform::builder()
            .name(self.config.name)
            .region(self.config.region)
            .artifact(self.artifact)
            .build()
            .await;
        Box::new(lambda)
    }
}

#[cfg(test)]
mod tests {
    use crate::{adapters::BoxedPlatform, artifacts::LambdaZip};
    use miette::{IntoDiagnostic, Result};
    use multitool_sdk::models::{
        PlatformConfig, PlatformConfigOneOf, PlatformConfigOneOfAwsLambda,
    };
    use serde_json::{Value, json};

    use super::PlatformBuilder;

    fn platform_json() -> Value {
        json!({
              "aws_lambda": {
                "name": "my-lambda-name",
                "region": "us-east-2"
              }
            }
        )
    }

    #[tokio::test]
    #[ignore = "not needed"]
    async fn dump_json() -> Result<()> {
        let platform = PlatformConfig::PlatformConfigOneOf(Box::new(PlatformConfigOneOf {
            aws_lambda: Box::new(PlatformConfigOneOfAwsLambda::new(
                "my-lambda-name".to_owned(),
                "us-east-2".to_owned(),
            )),
        }));
        println!("{}", serde_json::to_string_pretty(&platform).unwrap());
        assert!(false);
        Ok(())
    }

    #[tokio::test]
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
