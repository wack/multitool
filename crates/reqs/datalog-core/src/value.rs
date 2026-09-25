use std::cmp::Ordering;
use std::fmt::{self, Write};

/// A ground value.
///
/// A fact `p(a, b)` can itself be viewed as the value `Compound("p", [a, b])`,
/// and a zero-arity fact `q` as `Sym("q")`. This is what lets atoms be passed
/// as arguments to other predicates.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Value {
    Int(i64),
    Str(String),
    Sym(String),
    Compound(String, Vec<Value>),
}

impl Value {
    pub fn sym(s: impl Into<String>) -> Self {
        Value::Sym(s.into())
    }

    pub fn str(s: impl Into<String>) -> Self {
        Value::Str(s.into())
    }

    /// Functor name and arity for symbols (arity 0) and compounds.
    pub fn functor(&self) -> Option<(&str, usize)> {
        match self {
            Value::Sym(s) => Some((s, 0)),
            Value::Compound(f, args) => Some((f, args.len())),
            _ => None,
        }
    }

    /// Ordering used by `<`, `<=`, `>`, `>=` and `min`/`max`: only values of
    /// the same kind are comparable.
    pub fn compare(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Str(a), Value::Str(b)) => Some(a.cmp(b)),
            (Value::Sym(a), Value::Sym(b)) => Some(a.cmp(b)),
            (Value::Compound(..), Value::Compound(..)) => Some(self.cmp(other)),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(i) => write!(f, "{i}"),
            Value::Str(s) => write_quoted(f, s),
            Value::Sym(s) => f.write_str(s),
            Value::Compound(name, args) => {
                f.write_str(name)?;
                f.write_char('(')?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{a}")?;
                }
                f.write_char(')')
            }
        }
    }
}

pub(crate) fn write_quoted(f: &mut impl Write, s: &str) -> fmt::Result {
    f.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\t' => f.write_str("\\t")?,
            c => f.write_char(c)?,
        }
    }
    f.write_char('"')
}

/// Lowercase identifier: `[a-z][A-Za-z0-9_]*`.
pub fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_lowercase())
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
