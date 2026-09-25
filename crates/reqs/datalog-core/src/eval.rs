use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::ast::*;
use crate::check::Analysis;
use crate::value::Value;

pub type Tuple = Vec<Value>;
type Relations = HashMap<String, BTreeSet<Tuple>>;
type Bindings = HashMap<String, Value>;

/// One premise of a proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Premise {
    /// A positive body atom matched this fact.
    Fact(Fact),
    /// A negated body atom: no fact matched this pattern.
    Absent(String),
    /// Aggregates summarize many matches rather than listing them.
    Summary(String),
}

/// How a derived fact was first obtained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub rule: String,
    pub bindings: BTreeMap<String, Value>,
    pub premises: Vec<Premise>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Derivation {
    /// Evaluation round in which the fact first appeared. Lower rounds give
    /// shallower explanations.
    pub iteration: u32,
    pub proof: Proof,
}

/// The result of evaluation: every fact (asserted and derived), plus one
/// proof per derived fact. Asserted facts have no derivation.
///
/// Relations are ordered sets so evaluation, and therefore which proof is
/// recorded when several are found in the same round, is deterministic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Model {
    pub relations: HashMap<String, BTreeSet<Tuple>>,
    pub derived: HashMap<Fact, Derivation>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EvalError {
    #[error("rule `{rule}`: sum<{var}> needs integers, got `{value}`")]
    NonIntegerSum {
        rule: String,
        var: String,
        value: Value,
    },
    #[error("rule `{rule}`: integer overflow in sum<{var}>")]
    Overflow { rule: String, var: String },
    #[error("rule `{rule}`: min/max over values of different kinds")]
    Incomparable { rule: String },
    #[error("internal error: {0}")]
    Internal(String),
}

impl Model {
    pub fn tuples<'a>(&'a self, pred: &str) -> impl Iterator<Item = &'a Tuple> + 'a {
        self.relations.get(pred).into_iter().flatten()
    }

    pub fn contains(&self, f: &Fact) -> bool {
        self.relations
            .get(&f.pred)
            .is_some_and(|r| r.contains(&f.args))
    }

    pub fn derivation(&self, f: &Fact) -> Option<&Derivation> {
        self.derived.get(f)
    }

    /// All facts, sorted.
    pub fn facts(&self) -> Vec<Fact> {
        let mut out: Vec<Fact> = self
            .relations
            .iter()
            .flat_map(|(p, ts)| ts.iter().map(move |t| Fact::new(p.clone(), t.clone())))
            .collect();
        out.sort();
        out
    }

    /// Facts matching a pattern, with the variable bindings for each. Sorted.
    pub fn query(&self, pattern: &Atom) -> Vec<(Fact, BTreeMap<String, Value>)> {
        let mut out = Vec::new();
        for t in self.tuples(&pattern.pred) {
            if t.len() != pattern.args.len() {
                continue;
            }
            let mut b = Bindings::new();
            let mut trail = Vec::new();
            if pattern
                .args
                .iter()
                .zip(t)
                .all(|(p, v)| unify(p, v, &mut b, &mut trail))
            {
                let shown = b.into_iter().filter(|(k, _)| !is_wildcard(k)).collect();
                out.push((Fact::new(pattern.pred.clone(), t.clone()), shown));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

fn unify(t: &Term, v: &Value, b: &mut Bindings, trail: &mut Vec<String>) -> bool {
    match t {
        Term::Var(x) => match b.get(x) {
            Some(bound) => bound == v,
            None => {
                b.insert(x.clone(), v.clone());
                trail.push(x.clone());
                true
            }
        },
        Term::Const(c) => c == v,
        Term::Compound(f, args) => match v {
            Value::Compound(g, vs) if f == g && args.len() == vs.len() => {
                args.iter().zip(vs).all(|(a, x)| unify(a, x, b, trail))
            }
            _ => false,
        },
    }
}

fn undo(b: &mut Bindings, trail: &mut Vec<String>, mark: usize) {
    for x in trail.drain(mark..) {
        b.remove(&x);
    }
}

fn instantiate(t: &Term, b: &Bindings) -> Option<Value> {
    match t {
        Term::Var(x) => b.get(x).cloned(),
        Term::Const(v) => Some(v.clone()),
        Term::Compound(f, args) => Some(Value::Compound(
            f.clone(),
            args.iter()
                .map(|a| instantiate(a, b))
                .collect::<Option<_>>()?,
        )),
    }
}

/// Instantiate as far as possible, leaving unbound variables in place.
fn partial(t: &Term, b: &Bindings) -> Term {
    match t {
        Term::Var(x) => b.get(x).cloned().map_or_else(|| t.clone(), Term::Const),
        Term::Const(_) => t.clone(),
        Term::Compound(f, args) => {
            let args: Vec<Term> = args.iter().map(|a| partial(a, b)).collect();
            match args.iter().map(Term::ground).collect::<Option<Vec<_>>>() {
                Some(vs) => Term::Const(Value::Compound(f.clone(), vs)),
                None => Term::Compound(f.clone(), args),
            }
        }
    }
}

/// Execution order for a rule body: filters (negation, comparisons) run as
/// soon as their variables are bound; positive atoms otherwise keep source
/// order.
fn plan(body: &[Literal]) -> Vec<usize> {
    let mut bound = BTreeSet::new();
    let mut done = vec![false; body.len()];
    let mut order = Vec::with_capacity(body.len());
    while order.len() < body.len() {
        let ready = |l: &Literal, bound: &BTreeSet<String>| match l {
            Literal::Pos(_) => false,
            Literal::Neg(a) => a.vars().iter().all(|v| is_wildcard(v) || bound.contains(v)),
            Literal::Cmp(l, CmpOp::Eq, r) => l.vars().is_subset(bound) || r.vars().is_subset(bound),
            Literal::Cmp(..) => l.vars().is_subset(bound),
        };
        let pick = (0..body.len())
            .find(|&i| !done[i] && ready(&body[i], &bound))
            .or_else(|| (0..body.len()).find(|&i| !done[i] && matches!(body[i], Literal::Pos(_))))
            .or_else(|| (0..body.len()).find(|&i| !done[i]));
        let i = pick.expect("unfinished plan has a remaining literal");
        done[i] = true;
        order.push(i);
        match &body[i] {
            Literal::Pos(a) => bound.extend(a.vars()),
            Literal::Cmp(..) => bound.extend(body[i].vars()),
            Literal::Neg(_) => {}
        }
    }
    order
}

struct Solver<'a> {
    body: &'a [Literal],
    plan: &'a [usize],
    rels: &'a Relations,
    delta: Option<(usize, &'a BTreeSet<Tuple>)>,
    empty: &'a BTreeSet<Tuple>,
}

type Sink<'s> = dyn FnMut(&Bindings, &[Premise]) -> Result<(), EvalError> + 's;

impl Solver<'_> {
    fn run(
        &self,
        k: usize,
        b: &mut Bindings,
        trail: &mut Vec<String>,
        prem: &mut Vec<Premise>,
        out: &mut Sink<'_>,
    ) -> Result<(), EvalError> {
        let Some(&li) = self.plan.get(k) else {
            return out(b, prem);
        };
        match &self.body[li] {
            Literal::Pos(a) => {
                let src = match self.delta {
                    Some((di, d)) if di == li => d,
                    _ => self.rels.get(&a.pred).unwrap_or(self.empty),
                };
                for t in src {
                    let mark = trail.len();
                    if a.args.len() == t.len()
                        && a.args.iter().zip(t).all(|(p, v)| unify(p, v, b, trail))
                    {
                        prem.push(Premise::Fact(Fact::new(a.pred.clone(), t.clone())));
                        self.run(k + 1, b, trail, prem, out)?;
                        prem.pop();
                    }
                    undo(b, trail, mark);
                }
                Ok(())
            }
            Literal::Neg(a) => {
                let rel = self.rels.get(&a.pred).unwrap_or(self.empty);
                let exists = rel.iter().any(|t| {
                    let mark = trail.len();
                    let hit = a.args.iter().zip(t).all(|(p, v)| unify(p, v, b, trail));
                    undo(b, trail, mark);
                    hit
                });
                if exists {
                    return Ok(());
                }
                let shown = Atom {
                    pred: a.pred.clone(),
                    args: a.args.iter().map(|t| partial(t, b)).collect(),
                };
                prem.push(Premise::Absent(format!("not {shown}")));
                self.run(k + 1, b, trail, prem, out)?;
                prem.pop();
                Ok(())
            }
            Literal::Cmp(l, op, r) => {
                let mark = trail.len();
                let ok = match (instantiate(l, b), instantiate(r, b), op) {
                    (Some(x), Some(y), _) => compare(&x, *op, &y),
                    (Some(x), None, CmpOp::Eq) => unify(r, &x, b, trail),
                    (None, Some(y), CmpOp::Eq) => unify(l, &y, b, trail),
                    _ => {
                        return Err(EvalError::Internal(format!(
                            "unbound comparison `{}`",
                            self.body[li]
                        )));
                    }
                };
                if ok {
                    self.run(k + 1, b, trail, prem, out)?;
                }
                undo(b, trail, mark);
                Ok(())
            }
        }
    }
}

fn compare(x: &Value, op: CmpOp, y: &Value) -> bool {
    match op {
        CmpOp::Eq => x == y,
        CmpOp::Ne => x != y,
        _ => match x.compare(y) {
            None => false,
            Some(o) => match op {
                CmpOp::Lt => o == Ordering::Less,
                CmpOp::Le => o != Ordering::Greater,
                CmpOp::Gt => o == Ordering::Greater,
                CmpOp::Ge => o != Ordering::Less,
                CmpOp::Eq | CmpOp::Ne => unreachable!(),
            },
        },
    }
}

fn visible(b: &Bindings) -> BTreeMap<String, Value> {
    b.iter()
        .filter(|(k, _)| !is_wildcard(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

struct Evaluator<'a> {
    rels: Relations,
    derived: HashMap<Fact, Derivation>,
    round: u32,
    empty: BTreeSet<Tuple>,
    _p: std::marker::PhantomData<&'a ()>,
}

impl Evaluator<'_> {
    /// Evaluate one rule against the current relations, adding facts that
    /// are new to both `rels` and `fresh`.
    fn fire(
        &mut self,
        rule: &Rule,
        plan: &[usize],
        delta: Option<(usize, &BTreeSet<Tuple>)>,
        fresh: &mut Relations,
    ) -> Result<(), EvalError> {
        let solver = Solver {
            body: &rule.body,
            plan,
            rels: &self.rels,
            delta,
            empty: &self.empty,
        };
        let (mut b, mut trail, mut prem) = (Bindings::new(), Vec::new(), Vec::new());
        let round = self.round;

        if let Some((pos, func, var)) = rule.head.aggregate() {
            let mut groups: BTreeMap<Tuple, Vec<Value>> = BTreeMap::new();
            let mut group_bindings: HashMap<Tuple, BTreeMap<String, Value>> = HashMap::new();
            solver.run(0, &mut b, &mut trail, &mut prem, &mut |b, _| {
                let key = rule
                    .head
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        HeadArg::Term(t) => Some(instantiate(t, b)),
                        HeadArg::Agg(..) => None,
                    })
                    .collect::<Option<Tuple>>()
                    .ok_or_else(|| {
                        EvalError::Internal(format!("unbound head in `{}`", rule.name))
                    })?;
                let v = b.get(var).cloned().ok_or_else(|| {
                    EvalError::Internal(format!("unbound aggregate in `{}`", rule.name))
                })?;
                group_bindings.entry(key.clone()).or_insert_with(|| {
                    let head_vars = rule.head.vars();
                    visible(b)
                        .into_iter()
                        .filter(|(k, _)| head_vars.contains(k) && k != var)
                        .collect()
                });
                groups.entry(key).or_default().push(v);
                Ok(())
            })?;
            for (key, vals) in groups {
                let result = match func {
                    AggFn::Count => Value::Int(vals.len() as i64),
                    AggFn::Sum => {
                        let mut acc: i64 = 0;
                        for v in &vals {
                            let Value::Int(n) = v else {
                                return Err(EvalError::NonIntegerSum {
                                    rule: rule.name.clone(),
                                    var: var.to_string(),
                                    value: v.clone(),
                                });
                            };
                            acc = acc.checked_add(*n).ok_or_else(|| EvalError::Overflow {
                                rule: rule.name.clone(),
                                var: var.to_string(),
                            })?;
                        }
                        Value::Int(acc)
                    }
                    AggFn::Min | AggFn::Max => {
                        let mut best = vals[0].clone();
                        for v in &vals[1..] {
                            let o = v.compare(&best).ok_or_else(|| EvalError::Incomparable {
                                rule: rule.name.clone(),
                            })?;
                            if (func == AggFn::Min && o == Ordering::Less)
                                || (func == AggFn::Max && o == Ordering::Greater)
                            {
                                best = v.clone();
                            }
                        }
                        best
                    }
                };
                let mut tuple = key.clone();
                tuple.insert(pos, result);
                let proof = Proof {
                    rule: rule.name.clone(),
                    bindings: group_bindings.remove(&key).unwrap_or_default(),
                    premises: vec![Premise::Summary(format!(
                        "{func}<{var}> over {} match(es)",
                        vals.len()
                    ))],
                };
                record(
                    &self.rels,
                    fresh,
                    &mut self.derived,
                    &rule.head.pred,
                    tuple,
                    proof,
                    round,
                );
            }
            return Ok(());
        }

        let mut found: Vec<(Tuple, Proof)> = Vec::new();
        solver.run(0, &mut b, &mut trail, &mut prem, &mut |b, prem| {
            let tuple = rule
                .head
                .args
                .iter()
                .map(|a| match a {
                    HeadArg::Term(t) => instantiate(t, b),
                    HeadArg::Agg(..) => None,
                })
                .collect::<Option<Tuple>>()
                .ok_or_else(|| EvalError::Internal(format!("unbound head in `{}`", rule.name)))?;
            let known = self
                .rels
                .get(&rule.head.pred)
                .is_some_and(|r| r.contains(&tuple))
                || fresh
                    .get(&rule.head.pred)
                    .is_some_and(|r| r.contains(&tuple));
            if !known {
                found.push((
                    tuple,
                    Proof {
                        rule: rule.name.clone(),
                        bindings: visible(b),
                        premises: prem.to_vec(),
                    },
                ));
            }
            Ok(())
        })?;
        for (tuple, proof) in found {
            record(
                &self.rels,
                fresh,
                &mut self.derived,
                &rule.head.pred,
                tuple,
                proof,
                round,
            );
        }
        Ok(())
    }
}

fn record(
    rels: &Relations,
    fresh: &mut Relations,
    derived: &mut HashMap<Fact, Derivation>,
    pred: &str,
    tuple: Tuple,
    proof: Proof,
    iteration: u32,
) {
    if rels.get(pred).is_some_and(|r| r.contains(&tuple)) {
        return;
    }
    let set = fresh.entry(pred.to_string()).or_default();
    if set.insert(tuple.clone()) {
        derived.insert(Fact::new(pred, tuple), Derivation { iteration, proof });
    }
}

fn merge(rels: &mut Relations, fresh: &Relations) {
    for (p, ts) in fresh {
        rels.entry(p.clone())
            .or_default()
            .extend(ts.iter().cloned());
    }
}

/// Evaluate a program that passed [`crate::analyze`].
pub fn evaluate(p: &Program, a: &Analysis) -> Result<Model, EvalError> {
    let mut ev = Evaluator {
        rels: p
            .decls
            .iter()
            .map(|d| (d.name.clone(), BTreeSet::new()))
            .collect(),
        derived: HashMap::new(),
        round: 0,
        empty: BTreeSet::new(),
        _p: std::marker::PhantomData,
    };
    for f in &p.facts {
        ev.rels
            .entry(f.pred.clone())
            .or_default()
            .insert(f.args.clone());
    }

    for stratum in &a.strata {
        if stratum.rules.is_empty() {
            continue;
        }
        let rules: Vec<&Rule> = stratum.rules.iter().map(|&i| &p.rules[i]).collect();
        let plans: Vec<Vec<usize>> = rules.iter().map(|r| plan(&r.body)).collect();
        let in_stratum: HashSet<&str> = stratum.preds.iter().map(String::as_str).collect();

        // First round: every rule against everything known so far.
        ev.round += 1;
        let mut delta = Relations::new();
        for (r, pl) in rules.iter().zip(&plans) {
            ev.fire(r, pl, None, &mut delta)?;
        }
        merge(&mut ev.rels, &delta);

        if !stratum.recursive {
            continue;
        }
        // Semi-naive rounds: each recursive atom reads only last round's news.
        while delta.values().any(|d| !d.is_empty()) {
            ev.round += 1;
            let mut next = Relations::new();
            for (r, pl) in rules.iter().zip(&plans) {
                if r.head.aggregate().is_some() {
                    continue;
                }
                for (li, l) in r.body.iter().enumerate() {
                    let Literal::Pos(atom) = l else { continue };
                    if !in_stratum.contains(atom.pred.as_str()) {
                        continue;
                    }
                    if let Some(d) = delta.get(&atom.pred).filter(|d| !d.is_empty()) {
                        ev.fire(r, pl, Some((li, d)), &mut next)?;
                    }
                }
            }
            merge(&mut ev.rels, &next);
            delta = next;
        }
    }

    Ok(Model {
        relations: ev.rels,
        derived: ev.derived,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::analyze;
    use crate::parser::{Item, parse_atom, parse_fact, parse_program};

    fn run(src: &str) -> Model {
        let mut p = Program::default();
        for (i, item) in parse_program(src).unwrap().into_iter().enumerate() {
            match item {
                Item::Decl(d) => p.decls.push(d),
                Item::Rule(mut r) => {
                    if r.name.is_empty() {
                        r.name = format!("r{i}");
                    }
                    p.rules.push(r)
                }
                Item::Fact(f) => p.facts.push(f),
            }
        }
        let a = analyze(&p).unwrap();
        evaluate(&p, &a).unwrap()
    }

    fn has(m: &Model, f: &str) -> bool {
        m.contains(&parse_fact(f).unwrap())
    }

    #[test]
    fn transitive_closure() {
        let m = run(
            ".decl edge(a: symbol, b: symbol) .decl path(a: symbol, b: symbol)
             edge(a, b). edge(b, c). edge(c, d).
             base: path(X, Y) :- edge(X, Y).
             step: path(X, Z) :- path(X, Y), edge(Y, Z).",
        );
        assert!(has(&m, "path(a, d)"));
        assert!(!has(&m, "path(d, a)"));
        assert_eq!(m.tuples("path").count(), 6);
        let d = m.derivation(&parse_fact("path(a, d)").unwrap()).unwrap();
        assert_eq!(d.proof.rule, "step");
        assert_eq!(d.iteration, 3);
    }

    #[test]
    fn stratified_negation() {
        let m = run(
            ".decl node(x: symbol) .decl edge(a: symbol, b: symbol) .decl has_out(x: symbol) .decl sink(x: symbol)
             node(a). node(b). edge(a, b).
             has_out(X) :- edge(X, _).
             sink(X) :- node(X), not has_out(X).",
        );
        assert!(has(&m, "sink(b)"));
        assert!(!has(&m, "sink(a)"));
    }

    #[test]
    fn aggregates() {
        let m = run(".decl item(order: symbol, sku: symbol, price: int)
             .decl n(order: symbol, n: int) .decl total(order: symbol, t: int)
             .decl cheapest(order: symbol, p: int) .decl priciest(order: symbol, p: int)
             item(o1, a, 5). item(o1, b, 5). item(o1, c, 2). item(o2, a, 7).
             n(O, count<S>) :- item(O, S, _).
             total(O, sum<P>) :- item(O, _, P).
             cheapest(O, min<P>) :- item(O, _, P).
             priciest(O, max<P>) :- item(O, _, P).");
        assert!(has(&m, "n(o1, 3)"));
        assert!(has(&m, "total(o1, 12)"));
        assert!(has(&m, "cheapest(o1, 2)"));
        assert!(has(&m, "priciest(o2, 7)"));
    }

    #[test]
    fn compound_terms_and_equality() {
        let m = run(
            ".decl dep(a: symbol, b: symbol) .decl sat(a: term) .decl wrapped(x: term)
             dep(app, db).
             sat(node(db)).
             lift: sat(node(X)) :- dep(X, Y), sat(node(Y)).
             wrap: wrapped(W) :- sat(A), W = box(A).",
        );
        assert!(has(&m, "sat(node(app))"));
        assert!(has(&m, "wrapped(box(node(app)))"));
        let q = m.query(&parse_atom("sat(node(X))").unwrap());
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn comparisons() {
        let m = run(".decl n(x: int) .decl big(x: int) n(1). n(5). n(10).
             big(X) :- n(X), X >= 5, X != 10.");
        assert!(has(&m, "big(5)"));
        assert!(!has(&m, "big(10)"));
        assert!(!has(&m, "big(1)"));
    }
}
