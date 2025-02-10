use crate::stats::Observation;

pub use action::Action;
pub use chi::ChiSquareEngine;
pub use controller::EngineController;

/// The decision engine receives observations from the monitor
/// and determines whether the canary should be promoted, yanked,
/// or scaled up or down.
pub trait DecisionEngine<T: Observation> {
    /// [add_observation] provides a new observation that the engine
    /// should take under advisement before making a decision.
    fn add_observation(&mut self, observation: T);

    /// [compute] will ask the engine to run over all known observations.
    /// The engine isn't required to output an [Action]. It might determine
    /// there isn't enough data to make an affirmative decision.
    fn compute(&mut self) -> Option<Action>;
}

mod action;
mod chi;
mod controller;

/// The AlwaysPromote decision engine will always return the Promote
/// action when prompted. It discards all observations.
#[cfg(test)]
pub struct AlwaysPromote;

#[cfg(test)]
impl<T: Observation> DecisionEngine<T> for AlwaysPromote {
    fn add_observation(&mut self, _: T) {}

    fn compute(&mut self) -> Option<Action> {
        // true to its name, it will always promote the canary.
        Some(Action::Promote)
    }
}

#[cfg(test)]
mod tests {
    use super::{AlwaysPromote, DecisionEngine};
    use crate::{adapters::Action, metrics::ResponseStatusCode, stats::CategoricalObservation};
    use static_assertions::assert_obj_safe;

    type StatusCode = CategoricalObservation<5, ResponseStatusCode>;

    // We expect the DesignEngine to be boxed, and we expect
    // it to use response codes as input.
    assert_obj_safe!(DecisionEngine<ResponseStatusCode>);

    /// This test is mostly for TDD: I want need to see the DecisionEngine
    /// API is in action before I'm happy enough to move on to integrating
    /// it with the rest of the system.
    #[test]
    fn mock_decision_engine() {
        let mut engine: Box<dyn DecisionEngine<StatusCode>> = Box::new(AlwaysPromote);
        assert_eq!(Some(Action::Promote), engine.compute());
    }
}
