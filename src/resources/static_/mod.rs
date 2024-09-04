use std::error::Error;

use crate::state::{ResourcePrototype, ResourceRecord};
use async_trait::async_trait;
use aws_sdk_s3::{types::{BucketLocationConstraint, CreateBucketConfiguration}, Client as S3Client};
use serde_json::json;
use uuid::Uuid;

/// A (static) [Resource] defines all of the provider interactions
/// explicitly as methods.
type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[async_trait]
pub trait ResourceClass {
    // TODO: Can we remove the clone bound by using AsRef or a Where clause?
    //       This should be possible with GATs.
    type Proto: From<ResourcePrototype>;
    type Record: Into<ResourceRecord> + Clone;
   
    async fn create(&mut self, proto: Self::Proto) -> Result<Self::Record>;
    async fn delete(&mut self, record: Self::Record) -> Result<()>;
    async fn read(&mut self, record: Self::Proto) -> Result<Self::Record>;
    async fn update(&mut self, record: Self::Record) -> Result<Self::Record>;
}

/// A "Factory" for S3 bucket resources.
pub struct S3BucketClass {
    client: S3Client,
}

impl S3BucketClass {
    pub async fn new() -> Result<Self> {
        let config = aws_config::load_from_env().await;
        let client = aws_sdk_s3::Client::new(&config);
        Ok(Self {
            client,
        })
    }
}

#[derive(Clone)]
pub struct S3BucketPrototype {
    // TODO: This is ugly AF. I just copied over the fields from
    // the resource type. Lazy
    id: Uuid,
    inputs: serde_json::Value,
}

#[derive(Clone)]
pub struct S3BucketRecord {
    // TODO: This is ugly AF. I just copied over the fields from
    // the resource type. Lazy.
    id: Uuid,
    inputs: serde_json::Value,
    computed_fields: serde_json::Value,
}

impl S3BucketRecord {
    pub fn id(&self) -> Uuid {
        self.id
    } 

    pub fn inputs(&self) -> serde_json::Value {
        self.inputs.clone()
    }

    pub fn computed(&self) -> serde_json::Value {
        self.computed_fields.clone()
    }
}

impl From<ResourcePrototype> for S3BucketPrototype {
    fn from(value: ResourcePrototype) -> Self {
        Self {
            id: value.id(),
            inputs: value.inputs().clone(),
        }
    }
}

impl From<ResourceRecord> for S3BucketRecord {
    fn from(value: ResourceRecord) -> Self {
        Self {
            id: value.id().clone(),
            inputs: value.inputs().clone(),
            computed_fields: value.computed().clone(),
        }
    }
}

impl From<S3BucketRecord> for ResourceRecord {
    fn from(value: S3BucketRecord) -> Self {
        let proto = ResourcePrototype::new(value.id(), value.inputs().clone());
        Self::new(&proto, value.computed())
    }
}

#[async_trait]
impl ResourceClass for S3BucketClass {
    type Proto =  S3BucketPrototype;
    type Record = S3BucketRecord;

    async fn create(&mut self, proto: Self::Proto) -> Result<Self::Record> {
        let args = proto.inputs.as_object().expect("must be an object");
        let name = args.get("bucket_name").expect("json must have bucket name").as_str().expect("bucket_name must be a string").to_owned();
        let region = args.get("region").expect("json must have region").as_str().expect("region must be a string").to_owned();
        let constraint = BucketLocationConstraint::from(region.as_ref());
        let cfg = CreateBucketConfiguration::builder()
            .location_constraint(constraint)
            .build();
        self.client.create_bucket().bucket(name)
            .create_bucket_configuration(cfg)
            .send().await?;

        Ok(
            S3BucketRecord{
                id: proto.id,
                inputs: proto.inputs,
                computed_fields: json!({}),
            }
            
        )
    }

    async fn delete(&mut self, record: Self::Record) -> Result<()> {
        todo!()
    }

    async fn read(&mut self, record: Self::Proto) -> Result<Self::Record> {
        todo!()
    }

    async fn update(&mut self, record: Self::Record) -> Result<Self::Record> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use crate::state::ResourcePrototype;

    use super::S3BucketClass;
    use super::ResourceClass;
    use super::S3BucketPrototype;
    use super::S3BucketRecord;

    use static_assertions::assert_obj_safe;

    assert_obj_safe!(ResourceClass<Proto=S3BucketPrototype, Record=S3BucketRecord>);

    #[tokio::test]
    async fn test_s3_bucket() {
        let mut s3_factory = S3BucketClass::new().await.unwrap();
        let proto = ResourcePrototype::new(Uuid::new_v4(), json!({
            "bucket_name": "wack-multitool-demo-bucket",
            "region": "us-east-2",
        }));
        s3_factory.create(proto.into()).await.unwrap();    
    }
}
