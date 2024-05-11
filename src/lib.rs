trait Named {
    fn name(&self) -> &str;
}

impl<T: StaticallyNamed> Named for T {
    fn name(&self) -> &str {
        T::NAME
    }
}

trait StaticallyNamed {
    const NAME: &'static str;
}

/// A `CloudProvider` represents a single cloud.
trait CloudProvider: Named {
    // must be able to create an instance so we can access metadata like name().
    fn new() -> Self
    where
        Self: Sized;
}

impl StaticallyNamed for Aws {
    const NAME: &'static str = "aws";
}

struct Aws;
impl CloudProvider for Aws {
    fn new() -> Self
    where
        Self: Sized,
    {
        Self
    }
}

/// A `Platform` is somewhere code can be deployed e.g. Kubernetes,
/// a VM, or serverless
trait Platform: Named {}

struct Serverless;

impl StaticallyNamed for Serverless {
    const NAME: &'static str = "serverless";
}

impl Platform for Serverless {}

/// An adaptor contains the set of instructions to deploy an app
/// to a given `Platform` on a given `CloudProvider`.
trait Adaptor {
    /// each adaptor can self-describe the cloud for which it was built.
    fn cloud(&self) -> &dyn CloudProvider;

    /// each adaptor can self-describe the platform for which it was built.
    fn platform(&self) -> &dyn Platform;
}

#[cfg(test)]
mod tests {

    use super::{Adaptor, CloudProvider, Platform};
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Adaptor);
    assert_obj_safe!(CloudProvider);
    assert_obj_safe!(Platform);
}
