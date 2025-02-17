use std::fmt;

use crate::stats::Categorical;

/// [ResponseStatusCode] groups HTTP response status codes according
/// to five general categories. This type is used as the dependent
/// variable in statical observations.
#[derive(Hash, Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum ResponseStatusCode {
    // Information responses
    _1XX,
    // Successful responses
    _2XX,
    // Redirection messages
    _3XX,
    // Client error responses
    _4XX,
    // Server error responses
    _5XX,
}

impl Categorical<5> for ResponseStatusCode {
    fn category(&self) -> usize {
        match self {
            Self::_1XX => 0,
            Self::_2XX => 1,
            Self::_3XX => 2,
            Self::_4XX => 3,
            Self::_5XX => 4,
        }
    }
}

impl fmt::Display for ResponseStatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::_1XX => write!(f, "1XX"),
            Self::_2XX => write!(f, "2XX"),
            Self::_3XX => write!(f, "3XX"),
            Self::_4XX => write!(f, "4XX"),
            Self::_5XX => write!(f, "5XX"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ResponseStatusCode;
    use pretty_assertions::assert_str_eq;

    #[test]
    fn fmt_response_status_code() {
        let test_cases = [
            (ResponseStatusCode::_1XX, "1XX"),
            (ResponseStatusCode::_2XX, "2XX"),
            (ResponseStatusCode::_3XX, "3XX"),
            (ResponseStatusCode::_4XX, "4XX"),
            (ResponseStatusCode::_5XX, "5XX"),
        ];

        for (input, expected) in test_cases {
            let observed = input.to_string();
            assert_str_eq!(expected, observed);
        }
    }
}
