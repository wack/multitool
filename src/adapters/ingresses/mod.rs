use async_trait::async_trait;
use miette::Result;

// use crate::WholePercent;

pub use apig::AwsApiGateway;

/// Convenience alias since this type is often dynamically
/// dispatched.
pub type BoxIngress = Box<dyn Ingress>;

/// Ingresses are responsible for (1) controlling how much traffic the canary
/// gets (hence the name ingress, since it functions like a virtual LB) and
/// (2) deploying, yanking, and promoting both the canary and the baseline.
#[async_trait]
pub trait Ingress {
    /// Deploy the canary app. Do not assign it any traffic.
    async fn deploy(&mut self) -> Result<()>;
    async fn rollback_canary(&mut self) -> Result<()>;
    // async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()>;
    async fn promote_canary(&mut self) -> Result<()>;
}

mod apig;
