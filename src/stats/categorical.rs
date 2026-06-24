/// Data is [Categorical] if each element has a surjective mapping to a number
/// from `[0, N)`. The `[Categorical]` trait expresses data that fits into exactly one
/// of `N` categories (or bins). The value of `N` represents the total (i.e. the max)
/// number of categories.
/// For example, if modeling bools, the groups are `True` and `False`, so N=2.
/// If modeling a six sided die, the groups would be 0 through 5, so N=6.
/// Each instance must be able to report which category it belongs to (using Self::category method).
/// Categories are zero-indexed (the first category is represented by `0usize`).
/// You can think of a [Categorical] as a hashmap with fixed integer keys. When the map is
/// created, its keys must already be known and completely cover the range `[0, N)`.
///
/// `Categorical` lives in a crate-internal module, so this example is shown for
/// illustration only (it can't be a runnable doctest); the same logic is
/// exercised by a unit test below.
///
/// ```ignore
/// #[derive(PartialEq, Eq, Debug, Hash)]
/// enum Coin {
///     Heads,
///     Tails,
/// }
///
/// impl Categorical<2> for Coin {
///     fn category(&self) -> usize {
///         match self {
///             Self::Heads => 0,
///             Self::Tails => 1,
///         }
///     }
/// }
/// ```
pub trait Categorical<const N: usize> {
    fn category(&self) -> usize;
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_obj_safe;

    use super::Categorical;

    // The categorical trait must be object-safe.
    assert_obj_safe!(Categorical<5>);

    #[test]
    fn coin_maps_to_its_category() {
        #[derive(PartialEq, Eq, Debug, Hash)]
        enum Coin {
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

        assert_eq!(Coin::Heads.category(), 0);
        assert_eq!(Coin::Tails.category(), 1);
    }
}
