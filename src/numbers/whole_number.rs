use bigdecimal::ToPrimitive as _;
use std::fmt;

use super::FixedPrecisionNumber;

#[derive(Clone)]
pub struct WholeNumber(FixedPrecisionNumber<0>);

impl fmt::Display for WholeNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl WholeNumber {
    pub fn as_i32(self) -> i32 {
        self.0.to_i32().unwrap()
    }
}

impl From<FixedPrecisionNumber<0>> for WholeNumber {
    fn from(value: FixedPrecisionNumber<0>) -> Self {
        Self(value)
    }
}

impl From<i64> for WholeNumber {
    fn from(value: i64) -> Self {
        let fixed = FixedPrecisionNumber::from(value);
        Self::from(fixed)
    }
}

impl From<u64> for WholeNumber {
    fn from(value: u64) -> Self {
        let fixed = FixedPrecisionNumber::from(value);
        Self::from(fixed)
    }
}

impl From<i32> for WholeNumber {
    fn from(value: i32) -> Self {
        let fixed = FixedPrecisionNumber::from(value);
        Self::from(fixed)
    }
}

impl From<u32> for WholeNumber {
    fn from(value: u32) -> Self {
        let fixed = FixedPrecisionNumber::from(value);
        Self::from(fixed)
    }
}

impl From<f64> for WholeNumber {
    fn from(value: f64) -> Self {
        let fixed = FixedPrecisionNumber::from(value);
        Self::from(fixed)
    }
}

impl From<f32> for WholeNumber {
    fn from(value: f32) -> Self {
        let fixed = FixedPrecisionNumber::from(value);
        Self::from(fixed)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_str_eq;

    use super::WholeNumber;

    #[test]
    fn format_whole_number() {
        struct TestCase {
            input: i64,
            expected: &'static str,
        }
        let test_cases = [
            TestCase {
                input: -320,
                expected: "-320",
            },
            TestCase {
                input: 10101,
                expected: "10101",
            },
        ];
        for tc in test_cases {
            let observed = WholeNumber::from(tc.input).to_string();
            let expected = tc.expected;
            assert_str_eq!(expected, observed);
        }
    }
}
