use super::{group::Group, Categorical};

/// Marker trait. This marker trait is used to ensure concrete types satisfy
/// the expectations of the backend. The backend accepts
/// certain types of metrics only, like Response Codes, CPU, and Memory.
pub trait Observation {}

/// An [CategoricalObservation] represents a measured outcome binned into
/// one of a fixed number of categories. For example, response status codes can
/// be binned into 2XX (successes), 3XX (redirects), 4XX (client side errors), and 5XX
/// (server side errors). These four categories are enumerable (fixed in number).
#[derive(Clone)]
pub struct CategoricalObservation<const N: usize, Cat: Categorical<N>> {
    /// The experimental group or the control group.
    pub group: Group,
    /// The outcome of the observation, bucketed into a specific category.
    /// e.g. a response status code's highest order digit, 2XX, 5XX, etc.
    pub outcome: Cat,
}

// Implement the marker trait. CategoricalObservations are a type of observation.
impl<const N: usize, Cat: Categorical<N>> Observation for CategoricalObservation<N, Cat> {}

#[cfg(test)]
mod tests {
    use super::Observation;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Observation);
}
