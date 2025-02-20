use std::marker::PhantomData;

use super::Categorical;

/// [Histogram] is a data structure for tracking categorical observations.
/// In essence, it's a map from Category to count.
/// A [Histogram] has a fixed number of bins, each representing on of `N`
/// categories. Each time an member of the category is observed, it is
/// "added to the bin", incrementing the counter.
pub struct Histogram<const N: usize, C>
where
    C: Categorical<N>,
{
    /// Bins store the counts for each category. Since each category
    /// maps to a number 0..N, we use that number as an index into
    /// an array of integers, which store the count. In this way,
    /// we have a perfect hash from category => count.
    bins: Box<[u32; N]>,
    phantom: PhantomData<C>,
}

impl<const N: usize, C> Histogram<N, C>
where
    C: Categorical<N>,
{
    /// Create a new histogram with zeroes across the board.
    pub fn new() -> Self {
        let bins = Box::new([0; N]);
        Self {
            bins,
            phantom: PhantomData,
        }
    }

    /// Increment the observed count for the given category by 1.
    pub fn increment(&mut self, categorical: &C) {
        self.increment_by(categorical, 1);
    }

    /// Increment the observed count for the given category by `count`.
    pub fn increment_by(&mut self, categorical: &C, count: u32) {
        let index = categorical.category();
        self.bins[index] += count;
    }

    /// Return the count for the given category.
    pub fn get_count(&self, categorical: &C) -> u32 {
        let index = categorical.category();
        self.bins[index]
    }

    /// return the total number of observations in the histogram.
    /// This is the sum of the counts across all bins.
    pub fn total(&self) -> u32 {
        self.bins.iter().sum::<u32>()
    }

    /// Reset all bins to zero.
    pub fn clear(&mut self) {
        self.bins = Box::new([0; N]);
    }

    /// Reset the given bins to zero.
    fn clear_category(&mut self, cat: &C) {
        let index = cat.category();
        self.bins[index] = 0;
    }

    pub(super) fn set_count(&mut self, cat: &C, count: u32) {
        self.clear_category(cat);
        self.increment_by(cat, count);
    }

    pub(super) fn get_count_by_index(&self, i: usize) -> u32 {
        if i >= N {
            panic!(
                "Index out of bounds. The index provided must be a natural number less than the number of categories."
            );
        }
        self.bins[i]
    }
}

impl<const N: usize, C> Default for Histogram<N, C>
where
    C: Categorical<N>,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::stats::Categorical;

    use super::Histogram;

    impl Categorical<2> for bool {
        fn category(&self) -> usize {
            match self {
                true => 1,
                false => 0,
            }
        }
    }

    #[test]
    fn histogram_total() {
        let mut hist = Histogram::new();
        hist.increment_by(&true, 15);
        hist.increment_by(&false, 45);
        let expected = 15 + 45;
        let observed = hist.total();
        assert_eq!(expected, observed);
    }

    /// This simple smoke test demonstrates that we can enumerable
    /// simple categories, like booleans.
    #[test]
    fn boolean_categories() {
        assert_eq!(true.category(), 1);
        assert_eq!(false.category(), 0);
    }

    #[test]
    fn test_increment() {
        let mut hist = Histogram::new();
        // start at 0.
        assert_eq!(hist.get_count(&true), 0);
        assert_eq!(hist.get_count(&true), 0);
        hist.increment(&true);
        hist.increment(&true);
        hist.increment(&false);
        assert_eq!(hist.get_count(&true), 2);
        assert_eq!(hist.get_count(&false), 1);
    }
}
