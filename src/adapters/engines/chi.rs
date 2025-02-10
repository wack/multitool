use crate::pipeline::TimeoutBehavior;
use crate::{
    metrics::ResponseStatusCode,
    pipeline::{StageConfig, StageDetails},
    stats::{chi_square_test, CategoricalObservation, ContingencyTable, Group},
};
use tokio::time::Instant;

use super::{Action, DecisionEngine};

/// The [ChiSquareEngine] uses the Chi Square statistical
/// significance test to determine whether the canary should be promoted or not.
pub struct ChiSquareEngine {
    table: ContingencyTable<5, ResponseStatusCode>,
    stages: StageConfig,
    start_time: Instant,
}

impl ChiSquareEngine {
    pub fn new() -> Self {
        let start_time = Instant::now();
        Self {
            table: Default::default(),
            stages: Default::default(),
            start_time,
        }
    }

    pub fn reset_start_time(&mut self) {
        self.start_time = Instant::now();
    }

    fn advance(&mut self) -> Option<&StageDetails> {
        self.reset_start_time();
        self.stages.advance()
    }
}

impl DecisionEngine<CategoricalObservation<5, ResponseStatusCode>> for ChiSquareEngine {
    // TODO: From writing this method, it's apparent there should be a Vec implementation
    //       that adds Vec::len() to the total and concats the vectors together, because
    //       otherwise we're wasting a ton of cycles just incrementing counters.
    fn add_observation(&mut self, observation: CategoricalObservation<5, ResponseStatusCode>) {
        match observation.group {
            Group::Control => {
                // • Increment the number of observations for this category.
                self.table.increment_expected(&observation.outcome, 1);
            }
            Group::Experimental => {
                // • Increment the number of observations in the canary contingency table.
                self.table.increment_observed(&observation.outcome, 1);
            }
        }
    }

    fn compute(&mut self) -> Option<Action> {
        // • Check if we even have an active stage.
        let stage = self.stages.current()?;

        // • Otherwise, we know we can proceed with tabulation.
        // • Compute the p-value.
        let is_significant = chi_square_test(&self.table, stage.badness_confidence_limit());
        if is_significant {
            // Welp, it's time to roll back.
            // TODO: Must check to see if the canary is actually worse than the baseline.
            //       This will be a false positive if you're actually *fixing* a bug.
            return Some(Action::Rollback);
        }
        let timed_out = stage.has_timed_out(self.start_time);
        match (timed_out, stage.timeout_behavior()) {
            // If we've timed out, but there's no significant failure, then
            // we advance the stage.
            (true, TimeoutBehavior::Advance) => {
                let details = self.advance();
                match details {
                    Some(details) => Some(Action::RampTo(details.canary_traffic())),
                    None => Some(Action::Promote),
                }
            }
            // Otherwise, we keep observing.
            (false, TimeoutBehavior::Advance) => None,
        }
    }
}

impl Default for ChiSquareEngine {
    fn default() -> Self {
        Self::new()
    }
}
