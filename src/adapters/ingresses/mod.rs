use async_trait::async_trait;
use miette::Result;

use crate::{Shutdownable, WholePercent};

/// Convenience alias since this type is often dynamically
/// dispatched.
pub type BoxedIngress = Box<dyn Ingress + Send>;
pub use apig::AwsApiGateway;
pub use builder::IngressBuilder;

/// Ingresses are responsible for (1) controlling how much traffic the canary
/// gets (hence the name ingress, since it functions like a virtual LB) and
/// (2) deploying, yanking, and promoting both the canary and the baseline.
#[async_trait]
pub trait Ingress: Shutdownable {
    async fn set_canary_traffic(&mut self, percent: WholePercent) -> Result<()>;

    // TODO:
    // We might need the "promote" and "rollback" functions here too.
}

mod apig;
mod builder;

#[cfg(test)]
mod tests {
    use super::Ingress;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Ingress);
}
