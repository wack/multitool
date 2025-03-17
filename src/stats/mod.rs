pub use categorical::Categorical;
pub use group::Group;
pub use observation::{CategoricalObservation, Observation};

/// For modeling categorical data.
mod categorical;
mod contingency;
/// `group` defines the two groups.
mod group;
/// A data structure for tracking categorical data.
mod histogram;
/// An observation represents a group and the observed category.
mod observation;
