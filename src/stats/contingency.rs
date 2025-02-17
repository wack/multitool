use std::num::NonZeroUsize;

use crate::stats::{histogram::Histogram, Categorical};

/// A `ContingencyTable` is conceptually a two-dimensional table,
/// where each column represents a category, each row is a group (expected and observed),
/// and each cell is the count of observations.
/// Note, the number of elements in the expected row is used as a ratio to calculate
/// the actual expectation. For example, if you're flipping a fair coin, your expected
/// cells should be any matching number: i.e. 50/50, or 100/100 (a ratio equal to 1:1).
/// When a caller queries for the number of expected elements, the ratio from the expected
/// row is multiplied against the actual, total number of observed counts to determine the
/// expected number for the given category. For example, for a fair coin, if you flip it 30 times,
/// you'd multiply `30*50/(50+50)` to get `15`.
pub struct ContingencyTable<const N: usize, C: Categorical<N>> {
    expected: Histogram<N, C>,
    observed: Histogram<N, C>,
}

impl<const N: usize, C: Categorical<N>> ContingencyTable<N, C> {
    /// Create a new table with zeroes in each cell.
    pub fn new() -> Self {
        Self {
            expected: Default::default(),
            observed: Default::default(),
        }
    }

    /// Calculate the expected number of elements. This is a ratio
    pub fn expected(&self, cat: &C) -> f64 {
        let index = cat.category();
        self.expected_by_index(index)
    }

    /// calculate the expected count for the category with index `i`.
    pub fn expected_by_index(&self, i: usize) -> f64 {
        // • Calculate the expected number of elements as a ratio
        //   of the total number of elements observed.
        let expected_in_category = self.expected.get_count_by_index(i) as f64;
        let expected_total = self.expected.total() as f64;
        // • Grab the total number of elements observed, and calculate
        //   using the ratio.
        let total_observed = self.observed.total() as f64;
        // If nothing has been observed, then we expect zero observations.
        if total_observed == 0.0 || expected_in_category == 0.0 {
            return 0.0;
        }
        // Cast everything to a float since probabilities aren't always discrete.
        expected_in_category * total_observed / expected_total
    }

    /// calculate the observed count for the category with index `i`.
    pub fn observed_by_index(&self, i: usize) -> u32 {
        self.observed.get_count_by_index(i)
    }

    /// returns the number of degrees of freedom for this table.
    /// This is typically the number of categories minus one.
    /// # Panics
    /// This method panics if `N` is less than 2.
    pub fn degrees_of_freedom(&self) -> NonZeroUsize {
        if N < 2 {
            panic!("The experiment must have at least two groups. Only {N} groups provided");
        }
        NonZeroUsize::new(N - 1).unwrap()
    }

    pub fn observed(&self, cat: &C) -> u32 {
        self.observed.get_count(cat)
    }

    pub fn set_expected(&mut self, cat: &C, count: u32) {
        self.expected.set_count(cat, count);
    }

    pub fn set_observed(&mut self, cat: &C, count: u32) {
        self.observed.set_count(cat, count);
    }

    pub fn increment_expected(&mut self, cat: &C, count: u32) {
        self.expected.increment_by(cat, count);
    }

    pub fn increment_observed(&mut self, cat: &C, count: u32) {
        self.observed.increment_by(cat, count);
    }
}

impl<const N: usize, C: Categorical<N>> Default for ContingencyTable<N, C> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
pub(crate) use tests::Coin;

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use pretty_assertions::assert_eq;

    use super::ContingencyTable;

    /// This test exercises the ContingencyTable API when used for empirical
    /// observations, like those coming from a real-life webserver.
    /// This API updates values incrementally instead of setting them to a fixed value.
    #[test]
    fn empirical_expectations() {
        // Scenario:
        // • In the control group, we observe fifty 200 OK status codes and twenty 500 status codes.
        // • In the canary group, we observe ten 200 OK status codes and thirty 500 status codes.
        let mut table = ContingencyTable::new();
        // Done in two batches to exercise bin addition.
        table.increment_expected(&ResponseStatusCode::_2XX, 25);
        table.increment_expected(&ResponseStatusCode::_2XX, 25);
        table.increment_expected(&ResponseStatusCode::_5XX, 15);
        table.increment_expected(&ResponseStatusCode::_5XX, 5);

        table.increment_observed(&ResponseStatusCode::_2XX, 10);
        table.increment_observed(&ResponseStatusCode::_5XX, 30);
        // Assert the observations match.
        assert_eq!(table.observed(&ResponseStatusCode::_2XX), 10);
        assert_eq!(table.observed(&ResponseStatusCode::_5XX), 30);
        // Given that we have 70 expected observations, and 40 canary observations, we expect to
        // see 40*(50/70) 2XX status codes and 40*(20/70) 5XX status codes.
        let test_case_expected = 40.0 * 50.0 / 70.0;
        let test_case_observed = table.expected(&ResponseStatusCode::_2XX);
        assert_eq!(test_case_expected, test_case_observed);
        let test_case_expected = 40.0 * 20.0 / 70.0;
        let test_case_observed = table.expected(&ResponseStatusCode::_5XX);
        assert_eq!(test_case_expected, test_case_observed);
    }

    /// Test whether the ContingencyTable is able to correctly
    /// calculate the expected probabilities in a simple coin flip
    /// scenario.
    #[test]
    fn calculate_expected() {
        // Scenario: We want to test if a coin is fair.
        // Expected probability for each category is
        let mut table = ContingencyTable::new();
        // We expected an even number of heads and tails.
        // We don't have to use 50 here, as long as the numbers
        // are the same.
        table.set_expected(&Coin::Heads, 50);
        table.set_expected(&Coin::Tails, 50);

        table.set_observed(&Coin::Heads, 20);
        table.set_observed(&Coin::Tails, 80);
        // The coin should have a 50% of being either heads or tails.
        // Because there were 100 trials
        // in the observed group, we expect 50 = 100*50% Heads and Tails.
        assert_eq!(table.expected(&Coin::Heads), 50.0);
        assert_eq!(table.expected(&Coin::Tails), 50.0);
        // However, if we increase the number of observations to 1000, then
        // we'd expected 500 heads and 500 tails.
        table.set_observed(&Coin::Heads, 750);
        table.set_observed(&Coin::Tails, 250);
        assert_eq!(
            table.expected(&Coin::Heads),
            500.0,
            "expected 500 because the total is 1000"
        );
        assert_eq!(
            table.expected(&Coin::Tails),
            500.0,
            "expected 500 because the total is 1000"
        );
    }

    /// Demonstrate the default implementation to calculate
    /// degrees of freedom is correct.
    #[test]
    fn calc_degrees_of_freedom() {
        let table: ContingencyTable<2, Coin> = ContingencyTable::new();
        let expected = NonZeroUsize::new(1).unwrap();
        let observed = table.degrees_of_freedom();
        assert_eq!(observed, expected);
    }

    use crate::{metrics::ResponseStatusCode, stats::Categorical};
    #[derive(PartialEq, Eq, Debug, Hash)]
    pub(crate) enum Coin {
        Heads,
        Tails,
    }

    impl Categorical<2> for Coin {
        fn category(&self) -> usize {
            match self {
                Self::Heads => 0,
                Self::Tails => 1,
            }
        }
    }
}
