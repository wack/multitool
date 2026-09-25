use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use crate::ast::*;
use crate::value::is_ident;

/// One evaluation unit: a strongly connected component of the predicate
/// dependency graph, in dependency order.
#[derive(Clone, Debug)]
pub struct Stratum {
    pub preds: Vec<String>,
    /// Indices into `Program::rules` whose head is in this stratum.
    pub rules: Vec<usize>,
    /// Whether any predicate here depends on itself.
    pub recursive: bool,
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub strata: Vec<Stratum>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub struct CheckError(pub Vec<String>);

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join("\n"))
    }
}

/// Variables bound by a body: those in positive atoms, plus those bound
/// through `=` from an already-bound side.
pub fn bound_vars(body: &[Literal]) -> BTreeSet<String> {
    let mut bound = BTreeSet::new();
    for l in body {
        if let Literal::Pos(a) = l {
            bound.extend(a.vars());
        }
    }
    close_over_eq(body, &mut bound);
    bound
}

fn close_over_eq(body: &[Literal], bound: &mut BTreeSet<String>) {
    loop {
        let mut changed = false;
        for l in body {
            if let Literal::Cmp(l, CmpOp::Eq, r) = l {
                let (lv, rv) = (l.vars(), r.vars());
                if lv.is_subset(bound) && !rv.is_subset(bound) {
                    bound.extend(rv);
                    changed = true;
                } else if rv.is_subset(bound) && !lv.is_subset(bound) {
                    bound.extend(lv);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

fn show(vars: &BTreeSet<String>) -> String {
    vars.iter()
        .map(|v| {
            if is_wildcard(v) {
                "_".to_string()
            } else {
                v.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Validate a program and compute its evaluation order.
///
/// Checks: declarations exist and match arity and constant types; rule names
/// are unique identifiers; rules are safe (every variable in the head, in a
/// negated atom, or in a comparison is bound by a positive atom or `=`);
/// negation and aggregation are stratified; and recursive rules can't build
/// ever-larger compound terms.
pub fn analyze(p: &Program) -> Result<Analysis, CheckError> {
    let mut errs = Vec::new();
    let mut decls: HashMap<&str, &Decl> = HashMap::new();
    for d in &p.decls {
        if !is_ident(&d.name) {
            errs.push(format!("invalid predicate name `{}`", d.name));
        }
        if decls.insert(&d.name, d).is_some() {
            errs.push(format!("predicate `{}` is declared more than once", d.name));
        }
    }

    for f in &p.facts {
        match decls.get(f.pred.as_str()) {
            None => errs.push(format!(
                "fact `{f}`: predicate `{}` is not declared",
                f.pred
            )),
            Some(d) if d.arity() != f.args.len() => errs.push(format!(
                "fact `{f}`: `{}` takes {} argument(s), got {}",
                d.name,
                d.arity(),
                f.args.len()
            )),
            Some(d) => {
                for ((pname, ty), v) in d.params.iter().zip(&f.args) {
                    if !ty.admits(v) {
                        errs.push(format!("fact `{f}`: `{pname}` expects {ty}, got `{v}`"));
                    }
                }
            }
        }
    }

    let mut names = HashSet::new();
    for r in &p.rules {
        if !is_ident(&r.name) {
            errs.push(format!(
                "invalid rule name `{}` (use a lowercase identifier)",
                r.name
            ));
        }
        if !names.insert(r.name.as_str()) {
            errs.push(format!("rule name `{}` is used more than once", r.name));
        }
        check_rule(r, &decls, &mut errs);
    }
    if !errs.is_empty() {
        return Err(CheckError(errs));
    }

    let strata = stratify(p, &mut errs);
    if !errs.is_empty() {
        return Err(CheckError(errs));
    }
    Ok(Analysis { strata })
}

fn check_atom(ctx: &str, a: &Atom, decls: &HashMap<&str, &Decl>, errs: &mut Vec<String>) {
    let Some(d) = decls.get(a.pred.as_str()) else {
        errs.push(format!("{ctx}: predicate `{}` is not declared", a.pred));
        return;
    };
    if d.arity() != a.args.len() {
        errs.push(format!(
            "{ctx}: `{}` takes {} argument(s), got {}",
            d.name,
            d.arity(),
            a.args.len()
        ));
        return;
    }
    for ((pname, ty), t) in d.params.iter().zip(&a.args) {
        let ok = match t {
            Term::Var(_) => true,
            Term::Const(v) => ty.admits(v),
            Term::Compound(..) => *ty == Type::Term,
        };
        if !ok {
            errs.push(format!(
                "{ctx}: `{}` argument `{pname}` expects {ty}, got `{t}`",
                d.name
            ));
        }
    }
}

fn check_rule(r: &Rule, decls: &HashMap<&str, &Decl>, errs: &mut Vec<String>) {
    let ctx = format!("rule `{}`", r.name);
    let n_errs = errs.len();

    // Head.
    match decls.get(r.head.pred.as_str()) {
        None => errs.push(format!(
            "{ctx}: predicate `{}` is not declared",
            r.head.pred
        )),
        Some(d) if d.arity() != r.head.args.len() => errs.push(format!(
            "{ctx}: `{}` takes {} argument(s), got {}",
            d.name,
            d.arity(),
            r.head.args.len()
        )),
        Some(d) => {
            for ((pname, ty), a) in d.params.iter().zip(&r.head.args) {
                match a {
                    HeadArg::Term(t) => {
                        let ok = match t {
                            Term::Var(_) => true,
                            Term::Const(v) => ty.admits(v),
                            Term::Compound(..) => *ty == Type::Term,
                        };
                        if !ok {
                            errs.push(format!(
                                "{ctx}: head argument `{pname}` expects {ty}, got `{t}`"
                            ));
                        }
                    }
                    HeadArg::Agg(f, _) => {
                        let ok = match f {
                            AggFn::Count | AggFn::Sum => matches!(ty, Type::Int | Type::Term),
                            AggFn::Min | AggFn::Max => true,
                        };
                        if !ok {
                            errs.push(format!(
                                "{ctx}: `{f}` produces an int but `{pname}` expects {ty}"
                            ));
                        }
                    }
                }
            }
        }
    }
    if r.head
        .args
        .iter()
        .filter(|a| matches!(a, HeadArg::Agg(..)))
        .count()
        > 1
    {
        errs.push(format!("{ctx}: at most one aggregate per head"));
    }

    // Body atoms.
    for l in &r.body {
        if let Literal::Pos(a) | Literal::Neg(a) = l {
            check_atom(&ctx, a, decls, errs);
        }
    }
    if errs.len() > n_errs {
        return;
    }

    // Safety.
    let bound = bound_vars(&r.body);
    let unbound: BTreeSet<_> = r.head.vars().difference(&bound).cloned().collect();
    if !unbound.is_empty() {
        errs.push(format!(
            "{ctx}: head variable(s) {} must appear in a positive body atom",
            show(&unbound)
        ));
    }
    for l in &r.body {
        let free: BTreeSet<String> = match l {
            Literal::Pos(_) => continue,
            Literal::Neg(a) => a.vars().into_iter().filter(|v| !is_wildcard(v)).collect(),
            Literal::Cmp(..) => l.vars(),
        };
        let unbound: BTreeSet<_> = free.difference(&bound).cloned().collect();
        if !unbound.is_empty() {
            errs.push(format!(
                "{ctx}: variable(s) {} in `{l}` must be bound by a positive body atom",
                show(&unbound)
            ));
        }
    }
}

/// Tarjan's SCC over head → body edges, so components come out in
/// dependency order (dependencies first).
fn stratify(p: &Program, errs: &mut Vec<String>) -> Vec<Stratum> {
    let preds: Vec<&str> = p.decls.iter().map(|d| d.name.as_str()).collect();
    let idx: HashMap<&str, usize> = preds.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let n = preds.len();
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    for r in &p.rules {
        let h = idx[r.head.pred.as_str()];
        for l in &r.body {
            if let Literal::Pos(a) | Literal::Neg(a) = l {
                edges[h].push(idx[a.pred.as_str()]);
            }
        }
    }

    struct Tarjan<'a> {
        edges: &'a [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        out: Vec<Vec<usize>>,
    }
    impl Tarjan<'_> {
        fn visit(&mut self, v: usize) {
            self.index[v] = Some(self.next);
            self.low[v] = self.next;
            self.next += 1;
            self.stack.push(v);
            self.on_stack[v] = true;
            for i in 0..self.edges[v].len() {
                let w = self.edges[v][i];
                match self.index[w] {
                    None => {
                        self.visit(w);
                        self.low[v] = self.low[v].min(self.low[w]);
                    }
                    Some(iw) if self.on_stack[w] => self.low[v] = self.low[v].min(iw),
                    _ => {}
                }
            }
            if Some(self.low[v]) == self.index[v] {
                let mut comp = Vec::new();
                while let Some(w) = self.stack.pop() {
                    self.on_stack[w] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                self.out.push(comp);
            }
        }
    }
    let mut t = Tarjan {
        edges: &edges,
        index: vec![None; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        next: 0,
        out: Vec::new(),
    };
    for v in 0..n {
        if t.index[v].is_none() {
            t.visit(v);
        }
    }
    let comps = t.out;

    let mut comp_of = vec![0; n];
    for (c, members) in comps.iter().enumerate() {
        for &m in members {
            comp_of[m] = c;
        }
    }
    let mut recursive = vec![false; comps.len()];
    for (c, members) in comps.iter().enumerate() {
        recursive[c] = members.len() > 1 || members.iter().any(|&m| edges[m].contains(&m));
    }

    // Negation and aggregation must not occur inside a cycle.
    for r in &p.rules {
        let h = idx[r.head.pred.as_str()];
        let agg = r.head.aggregate().is_some();
        for l in &r.body {
            let (a, why) = match l {
                Literal::Neg(a) => (a, "negation"),
                Literal::Pos(a) if agg => (a, "aggregation"),
                _ => continue,
            };
            if comp_of[idx[a.pred.as_str()]] == comp_of[h] {
                errs.push(format!(
                    "rule `{}`: `{}` depends on `{}` through {why} inside a recursive cycle, \
                     which is not stratifiable",
                    r.name, r.head.pred, a.pred
                ));
            }
        }
    }

    // Recursive rules must not construct unboundedly deep terms.
    for r in &p.rules {
        let c = comp_of[idx[r.head.pred.as_str()]];
        if !recursive[c] {
            continue;
        }
        let mut outside = BTreeSet::new();
        for l in &r.body {
            if let Literal::Pos(a) = l
                && comp_of[idx[a.pred.as_str()]] != c
            {
                outside.extend(a.vars());
            }
        }
        close_over_eq(&r.body, &mut outside);
        let mut built: Vec<&Term> = r
            .head
            .args
            .iter()
            .filter_map(|a| match a {
                HeadArg::Term(t) if t.is_compound() => Some(t),
                _ => None,
            })
            .collect();
        for l in &r.body {
            if let Literal::Cmp(a, CmpOp::Eq, b) = l {
                built.extend([a, b].into_iter().filter(|t| t.is_compound()));
            }
        }
        for t in built {
            let bad: BTreeSet<_> = t.vars().difference(&outside).cloned().collect();
            if !bad.is_empty() {
                errs.push(format!(
                    "rule `{}`: compound term `{t}` in a recursive rule uses variable(s) {} \
                     bound only by the recursion, so evaluation might not terminate. \
                     Bind them with a non-recursive atom.",
                    r.name,
                    show(&bad)
                ));
            }
        }
    }

    comps
        .into_iter()
        .enumerate()
        .map(|(c, members)| {
            let names: HashSet<&str> = members.iter().map(|&m| preds[m]).collect();
            Stratum {
                preds: members.iter().map(|&m| preds[m].to_string()).collect(),
                rules: p
                    .rules
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| names.contains(r.head.pred.as_str()))
                    .map(|(i, _)| i)
                    .collect(),
                recursive: recursive[c],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Item, parse_program};

    fn program(src: &str) -> Program {
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
        p
    }

    #[test]
    fn rejects_unsafe_rules() {
        let e = analyze(&program(
            ".decl p(x: int) .decl q(x: int) p(X) :- not q(X).",
        ))
        .unwrap_err();
        assert!(e.0[0].contains("head variable"), "{e}");
    }

    #[test]
    fn rejects_unstratifiable_negation() {
        let e = analyze(&program(
            ".decl p(x: symbol) .decl q(x: symbol) .decl d(x: symbol)
             p(X) :- d(X), not q(X).  q(X) :- d(X), not p(X).",
        ))
        .unwrap_err();
        assert!(e.0[0].contains("not stratifiable"), "{e}");
    }

    #[test]
    fn rejects_recursive_aggregation() {
        let e = analyze(&program(".decl n(x: int) n(count<X>) :- n(X).")).unwrap_err();
        assert!(e.0[0].contains("aggregation"), "{e}");
    }

    #[test]
    fn rejects_term_growth() {
        let e = analyze(&program(".decl p(x: term) p(z). p(s(X)) :- p(X).")).unwrap_err();
        assert!(e.0[0].contains("might not terminate"), "{e}");
    }

    #[test]
    fn allows_terms_built_from_outside_the_cycle() {
        analyze(&program(
            ".decl sat(a: term) .decl dep(x: symbol, y: symbol)
             sat(leaf).
             sat(node(X)) :- dep(X, Y), sat(node(Y)).",
        ))
        .unwrap();
    }

    #[test]
    fn checks_types() {
        let e = analyze(&program(".decl p(x: int) p(\"no\").")).unwrap_err();
        assert!(e.0[0].contains("expects int"), "{e}");
    }
}
