//! A small Datalog engine.
//!
//! Features:
//! - typed declarations (`int`, `string`, `symbol`, `term`)
//! - compound terms (`f(a, X)`), with a finiteness check for recursive rules
//! - stratified negation (`not p(X)`) and comparison builtins (`=`, `!=`, `<`, …)
//! - head aggregates (`count<X>`, `sum<X>`, `min<X>`, `max<X>`), stratified
//! - semi-naive bottom-up evaluation with one proof recorded per derived fact
//! - a [`Store`] trait so persistence lives outside this crate

pub mod ast;
pub mod check;
pub mod eval;
pub mod parser;
pub mod store;
pub mod value;

pub use ast::{AggFn, Atom, CmpOp, Decl, Fact, Head, HeadArg, Literal, Program, Rule, Term, Type};
pub use check::{Analysis, CheckError, Stratum, analyze};
pub use eval::{Derivation, EvalError, Model, Premise, Proof, Tuple, evaluate};
pub use parser::{Item, ParseError};
pub use store::{Change, ModelSnapshot, Store};
pub use value::Value;
