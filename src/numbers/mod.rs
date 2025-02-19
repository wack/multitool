use bigdecimal::{BigDecimal, FromPrimitive, RoundingMode};
use std::fmt;

pub(crate) use errors::OutOfRangeError;

pub type CpuUsage = FixedPrecisionNumber<2>;
pub use whole_number::WholeNumber;

mod errors;

pub struct WholePercent(WholeNumber);

impl TryFrom<WholeNumber> for WholePercent {
    type Error = OutOfRangeError;
    fn try_from(value: WholeNumber) -> Result<Self, Self::Error> {
        let greater_than_zero = 0 <= value.clone().as_i32();
        let less_than_one_hundred = value.clone().as_i32() < 100;
        if greater_than_zero && less_than_one_hundred {
            Ok(Self(value))
        } else {
            Err(OutOfRangeError)
        }
    }
}

impl TryFrom<i64> for WholePercent {
    type Error = OutOfRangeError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::try_from(WholeNumber::from(value))
    }
}

impl TryFrom<u64> for WholePercent {
    type Error = OutOfRangeError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::try_from(WholeNumber::from(value))
    }
}

impl TryFrom<f64> for WholePercent {
    type Error = OutOfRangeError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        Self::try_from(WholeNumber::from(value))
    }
}

impl TryFrom<f32> for WholePercent {
    type Error = OutOfRangeError;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        Self::try_from(WholeNumber::from(value))
    }
}

impl TryFrom<i32> for WholePercent {
    type Error = OutOfRangeError;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Self::try_from(WholeNumber::from(value))
    }
}

impl TryFrom<u32> for WholePercent {
    type Error = OutOfRangeError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::try_from(WholeNumber::from(value))
    }
}

/// Unlike floating point numbers, which have floating percision,
/// this number has fixed percision.
#[derive(Clone)]
pub struct FixedPrecisionNumber<const SCALE: usize>(BigDecimal);

// TODO: we probably have to override the default implementaiton
//       of to_f64 and to_f32 because they truncate the decimal
//       part of the number.
impl<const SCALE: usize> bigdecimal::ToPrimitive for FixedPrecisionNumber<SCALE> {
    fn to_i64(&self) -> Option<i64> {
        self.0.to_i64()
    }

    fn to_u64(&self) -> Option<u64> {
        self.0.to_u64()
    }
}

impl<const SCALE: usize> From<BigDecimal> for FixedPrecisionNumber<SCALE> {
    fn from(n: BigDecimal) -> Self {
        Self(n.with_scale_round(SCALE as i64, RoundingMode::HalfUp))
    }
}

impl<const SCALE: usize> From<f64> for FixedPrecisionNumber<SCALE> {
    fn from(n: f64) -> Self {
        let num = BigDecimal::from_f64(n).unwrap();
        Self::from(num)
    }
}

impl<const SCALE: usize> From<f32> for FixedPrecisionNumber<SCALE> {
    fn from(n: f32) -> Self {
        let num = BigDecimal::from_f32(n).unwrap();
        Self::from(num)
    }
}

impl<const SCALE: usize> From<u64> for FixedPrecisionNumber<SCALE> {
    fn from(n: u64) -> Self {
        let num = BigDecimal::from_u64(n).unwrap();
        Self::from(num)
    }
}

impl<const SCALE: usize> From<u32> for FixedPrecisionNumber<SCALE> {
    fn from(n: u32) -> Self {
        let num = BigDecimal::from_u32(n).unwrap();
        Self::from(num)
    }
}

impl<const SCALE: usize> From<i64> for FixedPrecisionNumber<SCALE> {
    fn from(n: i64) -> Self {
        let num = BigDecimal::from_i64(n).unwrap();
        Self::from(num)
    }
}

impl<const SCALE: usize> From<i32> for FixedPrecisionNumber<SCALE> {
    fn from(n: i32) -> Self {
        let num = BigDecimal::from_i32(n).unwrap();
        Self::from(num)
    }
}

impl<const SCALE: usize> fmt::Display for FixedPrecisionNumber<SCALE> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

mod whole_number;

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_str_eq;

    use super::FixedPrecisionNumber;

    #[test]
    fn format_2_decimals() {
        struct TestCase {
            input: f32,
            expected: &'static str,
        }
        let test_cases = [
            TestCase {
                input: 0.4,
                expected: "0.40",
            },
            TestCase {
                input: 1001.1,
                // 0.1 cannot be perfectly represented by floating points,
                // so the input is actually 1001.099999… but we round it
                // to two decimals during construction.
                expected: "1001.10",
            },
            TestCase {
                input: 54995.99911,
                expected: "54996.00",
            },
        ];
        for tc in test_cases {
            let observed = FixedPrecisionNumber::<2>::from(tc.input).to_string();
            let expected = tc.expected;
            assert_str_eq!(expected, observed);
        }
    }
}
