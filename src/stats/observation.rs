use std::fmt::Debug;

use super::{Categorical, group::Group, histogram::Histogram};
use std::fmt;

/// Marker trait. This marker trait is used to ensure concrete types satisfy
/// the expectations of the backend. The backend accepts
/// certain types of metrics only, like Response Codes, CPU, and Memory.
pub trait Observation: Debug {}

// Implement the marker trait. CategoricalObservations are a type of observation.
/// An [CategoricalObservation] represents a measured outcome binned into
/// one of a fixed number of categories. For example, response status codes can
/// be binned into 2XX (successes), 3XX (redirects), 4XX (client side errors), and 5XX
/// (server side errors). These four categories are enumerable (fixed in number).
/// This type uses the index into the index into the array to indicate
/// the outcome.
/// For example, for a CoinFlip outcome (Heads vs. Tails), the variant Heads=0
/// and Tails=1 by assignment in the type Cat. Therefore, we store the number
/// of observed Heads in the array[0] and the number of observed tails in the array[1].
#[derive(Clone)]
pub struct CategoricalObservation<const N: usize, Cat: Categorical<N>> {
    /// The experimental group or the control group.
    group: Group,
    /// The outcome of the observation, bucketed into a specific category.
    /// e.g. a response status code's highest order digit, 2XX, 5XX, etc.
    histogram: Histogram<N, Cat>,
}

impl<const N: usize, Cat: Categorical<N>> CategoricalObservation<N, Cat> {
    /// Create a new (empty) categorical observation. All bins are set to 0.
    pub fn new(group: Group) -> Self {
        Self {
            group,
            histogram: Histogram::default(),
        }
    }

    /// Increase the stored count for the given category by the given number.
    pub fn increment_by(&mut self, category: &Cat, count: u32) {
        self.histogram.increment_by(category, count);
    }

    pub fn group(&self) -> Group {
        self.group
    }

    pub fn get_count(&self, cat: &Cat) -> u32 {
        self.histogram.get_count(cat)
    }
}

impl<const N: usize, Cat: Categorical<N> + fmt::Debug> fmt::Debug
    for CategoricalObservation<N, Cat>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Group: {:?}, Outcome: {:?}", self.group, self.histogram)
    }
}

// Implement the marker trait. CategoricalObservations are a type of observation.
impl<const N: usize, Cat: Categorical<N> + fmt::Debug> Observation
    for CategoricalObservation<N, Cat>
{
}

#[cfg(test)]
mod tests {
    use super::Observation;
    use static_assertions::assert_obj_safe;

    assert_obj_safe!(Observation);
}
