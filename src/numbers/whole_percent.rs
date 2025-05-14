use crate::numbers::WholeNumber;
use std::fmt;

use super::OutOfRangeError;

#[derive(Clone)]
pub struct WholePercent(WholeNumber);

impl fmt::Display for WholePercent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}%", self.0)
    }
}

impl WholePercent {
    pub fn as_i32(self) -> i32 {
        self.0.as_i32()
    }

    pub fn as_u64(self) -> u64 {
        self.0.as_u64()
    }
}

impl TryFrom<WholeNumber> for WholePercent {
    type Error = OutOfRangeError;
    fn try_from(value: WholeNumber) -> Result<Self, Self::Error> {
        let greater_than_zero = 0 <= value.clone().as_i32();
        let less_than_one_hundred = value.clone().as_i32() <= 100;
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

#[cfg(test)]
mod tests {
    use super::{OutOfRangeError, WholePercent};

    #[test]
    fn format_whole_percent() -> Result<(), OutOfRangeError> {
        struct TestCase {
            input: i32,
            expected: Result<&'static str, OutOfRangeError>,
        }
        let test_cases = [
            TestCase {
                input: 0,
                expected: Ok("0%"),
            },
            TestCase {
                input: -1,
                expected: Err(OutOfRangeError),
            },
            TestCase {
                input: 100,
                expected: Ok("100%"),
            },
            TestCase {
                input: 99,
                expected: Ok("99%"),
            },
            TestCase {
                input: 10101,
                expected: Err(OutOfRangeError),
            },
        ];
        for tc in test_cases {
            let observed = WholePercent::try_from(tc.input).map(|val| val.to_string());
            let expected = tc.expected.map(ToString::to_string);
            assert_eq!(expected, observed);
        }
        Ok(())
    }
}
