use std::collections::BTreeSet;
use std::fmt::{self, Write};

use crate::store::Change;
use crate::value::Value;

/// Parameter type in a declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Int,
    String,
    Symbol,
    /// Any value, including compound terms.
    Term,
}

impl Type {
    pub fn parse(s: &str) -> Option<Type> {
        match s {
            "int" => Some(Type::Int),
            "string" => Some(Type::String),
            "symbol" => Some(Type::Symbol),
            "term" => Some(Type::Term),
            _ => None,
        }
    }

    pub fn admits(self, v: &Value) -> bool {
        matches!(
            (self, v),
            (Type::Term, _)
                | (Type::Int, Value::Int(_))
                | (Type::String, Value::Str(_))
                | (Type::Symbol, Value::Sym(_))
        )
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Type::Int => "int",
            Type::String => "string",
            Type::Symbol => "symbol",
            Type::Term => "term",
        })
    }
}

/// `.decl name(param: type, ...) @annotation ...`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decl {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub annotations: Vec<String>,
}

impl Decl {
    pub fn arity(&self) -> usize {
        self.params.len()
    }

    pub fn has_annotation(&self, a: &str) -> bool {
        self.annotations.iter().any(|x| x == a)
    }
}

/// A term in a rule: variable, constant, or compound with (possibly
/// non-ground) arguments.
///
/// Wildcards (`_`) are parsed as uniquely named variables starting with `_#`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Term {
    Var(String),
    Const(Value),
    Compound(String, Vec<Term>),
}

pub fn is_wildcard(var: &str) -> bool {
    var.starts_with("_#")
}

impl Term {
    pub fn vars_into(&self, out: &mut BTreeSet<String>) {
        match self {
            Term::Var(v) => {
                out.insert(v.clone());
            }
            Term::Const(_) => {}
            Term::Compound(_, args) => args.iter().for_each(|a| a.vars_into(out)),
        }
    }

    pub fn vars(&self) -> BTreeSet<String> {
        let mut s = BTreeSet::new();
        self.vars_into(&mut s);
        s
    }

    /// The value of this term if it contains no variables.
    pub fn ground(&self) -> Option<Value> {
        match self {
            Term::Var(_) => None,
            Term::Const(v) => Some(v.clone()),
            Term::Compound(f, args) => Some(Value::Compound(
                f.clone(),
                args.iter().map(Term::ground).collect::<Option<_>>()?,
            )),
        }
    }

    pub fn is_compound(&self) -> bool {
        matches!(self, Term::Compound(..) | Term::Const(Value::Compound(..)))
    }
}

impl From<Value> for Term {
    fn from(v: Value) -> Self {
        Term::Const(v)
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Term::Var(v) if is_wildcard(v) => f.write_char('_'),
            Term::Var(v) => f.write_str(v),
            Term::Const(v) => write!(f, "{v}"),
            Term::Compound(name, args) => {
                f.write_str(name)?;
                f.write_char('(')?;
                write_list(f, args)?;
                f.write_char(')')
            }
        }
    }
}

fn write_list<T: fmt::Display>(f: &mut impl Write, xs: &[T]) -> fmt::Result {
    for (i, x) in xs.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{x}")?;
    }
    Ok(())
}

/// `pred(arg, ...)`, or just `pred` for arity 0.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Atom {
    pub pred: String,
    pub args: Vec<Term>,
}

impl Atom {
    pub fn vars(&self) -> BTreeSet<String> {
        let mut s = BTreeSet::new();
        self.args.iter().for_each(|a| a.vars_into(&mut s));
        s
    }

    /// The atom viewed as a term: `p(a, X)` → `Compound("p", [a, X])`, `q` → `Sym("q")`.
    pub fn to_term(&self) -> Term {
        if self.args.is_empty() {
            Term::Const(Value::Sym(self.pred.clone()))
        } else {
            Term::Compound(self.pred.clone(), self.args.clone())
        }
    }

    /// Inverse of [`Atom::to_term`].
    pub fn from_term(t: &Term) -> Option<Atom> {
        match t {
            Term::Const(Value::Sym(s)) => Some(Atom {
                pred: s.clone(),
                args: vec![],
            }),
            Term::Const(Value::Compound(f, args)) => Some(Atom {
                pred: f.clone(),
                args: args.iter().cloned().map(Term::Const).collect(),
            }),
            Term::Compound(f, args) => Some(Atom {
                pred: f.clone(),
                args: args.clone(),
            }),
            _ => None,
        }
    }
}

impl fmt::Display for Atom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.pred)?;
        if !self.args.is_empty() {
            f.write_char('(')?;
            write_list(f, &self.args)?;
            f.write_char(')')?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl fmt::Display for CmpOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Literal {
    Pos(Atom),
    Neg(Atom),
    Cmp(Term, CmpOp, Term),
}

impl Literal {
    pub fn vars(&self) -> BTreeSet<String> {
        match self {
            Literal::Pos(a) | Literal::Neg(a) => a.vars(),
            Literal::Cmp(l, _, r) => {
                let mut s = l.vars();
                r.vars_into(&mut s);
                s
            }
        }
    }
}

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Pos(a) => write!(f, "{a}"),
            Literal::Neg(a) => write!(f, "not {a}"),
            Literal::Cmp(l, op, r) => write!(f, "{l} {op} {r}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AggFn {
    Count,
    Sum,
    Min,
    Max,
}

impl AggFn {
    pub fn parse(s: &str) -> Option<AggFn> {
        match s {
            "count" => Some(AggFn::Count),
            "sum" => Some(AggFn::Sum),
            "min" => Some(AggFn::Min),
            "max" => Some(AggFn::Max),
            _ => None,
        }
    }
}

impl fmt::Display for AggFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AggFn::Count => "count",
            AggFn::Sum => "sum",
            AggFn::Min => "min",
            AggFn::Max => "max",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HeadArg {
    Term(Term),
    /// `count<X>` etc. Grouped by the other head arguments.
    Agg(AggFn, String),
}

impl fmt::Display for HeadArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeadArg::Term(t) => write!(f, "{t}"),
            HeadArg::Agg(func, v) => write!(f, "{func}<{v}>"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Head {
    pub pred: String,
    pub args: Vec<HeadArg>,
}

impl Head {
    pub fn aggregate(&self) -> Option<(usize, AggFn, &str)> {
        self.args.iter().enumerate().find_map(|(i, a)| match a {
            HeadArg::Agg(f, v) => Some((i, *f, v.as_str())),
            HeadArg::Term(_) => None,
        })
    }

    /// The head as a plain atom, if it has no aggregate.
    pub fn as_atom(&self) -> Option<Atom> {
        Some(Atom {
            pred: self.pred.clone(),
            args: self
                .args
                .iter()
                .map(|a| match a {
                    HeadArg::Term(t) => Some(t.clone()),
                    HeadArg::Agg(..) => None,
                })
                .collect::<Option<_>>()?,
        })
    }

    pub fn vars(&self) -> BTreeSet<String> {
        let mut s = BTreeSet::new();
        for a in &self.args {
            match a {
                HeadArg::Term(t) => t.vars_into(&mut s),
                HeadArg::Agg(_, v) => {
                    s.insert(v.clone());
                }
            }
        }
        s
    }
}

impl From<Atom> for Head {
    fn from(a: Atom) -> Self {
        Head {
            pred: a.pred,
            args: a.args.into_iter().map(HeadArg::Term).collect(),
        }
    }
}

impl fmt::Display for Head {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.pred)?;
        if !self.args.is_empty() {
            f.write_char('(')?;
            write_list(f, &self.args)?;
            f.write_char(')')?;
        }
        Ok(())
    }
}

/// `@annotation name: head :- body.`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub name: String,
    pub annotations: Vec<String>,
    pub head: Head,
    pub body: Vec<Literal>,
}

impl Rule {
    pub fn has_annotation(&self, a: &str) -> bool {
        self.annotations.iter().any(|x| x == a)
    }

    /// Multi-line rendering for display and export. [`fmt::Display`] gives the
    /// canonical single-line form.
    pub fn pretty(&self) -> String {
        let mut s = String::new();
        for a in &self.annotations {
            let _ = write!(s, "@{a} ");
        }
        if !self.annotations.is_empty() {
            s.pop();
            s.push('\n');
        }
        let _ = write!(s, "{}: {}", self.name, self.head);
        let one_line: Vec<String> = self.body.iter().map(|l| l.to_string()).collect();
        let joined = one_line.join(", ");
        if self.body.is_empty() {
            s.push('.');
        } else if self.body.len() <= 2 && s.len() + joined.len() < 88 {
            let _ = write!(s, " :- {joined}.");
        } else {
            s.push_str(" :-\n    ");
            s.push_str(&one_line.join(",\n    "));
            s.push('.');
        }
        s
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for a in &self.annotations {
            write!(f, "@{a} ")?;
        }
        write!(f, "{}: {}", self.name, self.head)?;
        if !self.body.is_empty() {
            f.write_str(" :- ")?;
            write_list(f, &self.body)?;
        }
        f.write_char('.')
    }
}

impl fmt::Display for Decl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ".decl {}(", self.name)?;
        for (i, (n, t)) in self.params.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{n}: {t}")?;
        }
        f.write_char(')')?;
        for a in &self.annotations {
            write!(f, " @{a}")?;
        }
        Ok(())
    }
}

/// A ground atom.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fact {
    pub pred: String,
    pub args: Vec<Value>,
}

impl Fact {
    pub fn new(pred: impl Into<String>, args: Vec<Value>) -> Self {
        Fact {
            pred: pred.into(),
            args,
        }
    }

    /// The fact viewed as a value (see [`Value`]).
    pub fn to_value(&self) -> Value {
        if self.args.is_empty() {
            Value::Sym(self.pred.clone())
        } else {
            Value::Compound(self.pred.clone(), self.args.clone())
        }
    }

    pub fn from_value(v: &Value) -> Option<Fact> {
        match v {
            Value::Sym(s) => Some(Fact::new(s.clone(), vec![])),
            Value::Compound(f, args) => Some(Fact::new(f.clone(), args.clone())),
            _ => None,
        }
    }
}

impl fmt::Display for Fact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_value())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    pub decls: Vec<Decl>,
    pub rules: Vec<Rule>,
    pub facts: Vec<Fact>,
}

impl Program {
    pub fn decl(&self, name: &str) -> Option<&Decl> {
        self.decls.iter().find(|d| d.name == name)
    }

    pub fn rule(&self, name: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.name == name)
    }

    pub fn has_fact(&self, f: &Fact) -> bool {
        self.facts.contains(f)
    }

    /// Apply a change in memory. Returns whether anything changed.
    pub fn apply(&mut self, change: &Change) -> bool {
        match change {
            Change::PutDecl(d) => match self.decls.iter_mut().find(|x| x.name == d.name) {
                Some(x) if x == d => false,
                Some(x) => {
                    *x = d.clone();
                    true
                }
                None => {
                    self.decls.push(d.clone());
                    true
                }
            },
            Change::RemoveDecl(n) => {
                let before = self.decls.len();
                self.decls.retain(|d| &d.name != n);
                before != self.decls.len()
            }
            Change::PutRule(r) => match self.rules.iter_mut().find(|x| x.name == r.name) {
                Some(x) if x == r => false,
                Some(x) => {
                    *x = r.clone();
                    true
                }
                None => {
                    self.rules.push(r.clone());
                    true
                }
            },
            Change::RemoveRule(n) => {
                let before = self.rules.len();
                self.rules.retain(|r| &r.name != n);
                before != self.rules.len()
            }
            Change::RenameRule { from, to } => {
                match self.rules.iter_mut().find(|r| &r.name == from) {
                    Some(r) => {
                        r.name = to.clone();
                        true
                    }
                    None => false,
                }
            }
            Change::AddFact(f) => {
                if self.facts.contains(f) {
                    false
                } else {
                    self.facts.push(f.clone());
                    true
                }
            }
            Change::RemoveFact(f) => {
                let before = self.facts.len();
                self.facts.retain(|x| x != f);
                before != self.facts.len()
            }
        }
    }

    /// Source text for the whole program, suitable for re-parsing.
    pub fn to_source(&self) -> String {
        let mut s = String::new();
        for d in &self.decls {
            let _ = writeln!(s, "{d}");
        }
        if !self.rules.is_empty() {
            s.push('\n');
        }
        for r in &self.rules {
            let _ = writeln!(s, "{}\n", r.pretty());
        }
        let mut facts = self.facts.clone();
        facts.sort();
        for f in &facts {
            let _ = writeln!(s, "{f}.");
        }
        s
    }
}
