use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::OnceLock;

use datalog_core::ast::{Atom, Decl, Fact, Head, HeadArg, Literal, Program, Rule, Term};
use datalog_core::check::{Analysis, analyze, bound_vars};
use datalog_core::parser::{Item, parse_program};
use datalog_core::value::Value;

/// Annotation marking requirement predicates and requirement rules.
pub const REQUIREMENT: &str = "requirement";

/// Declarations and rules the requirements layer adds to every program.
pub const PRELUDE: &str = r#"
.decl sat(atom: term)
.decl candidate(derivation: term, atom: term)
.decl child(derivation: term, atom: term)
.decl required(atom: term)
.decl excluded(atom: term)
.decl evidence(atom: term, source: string)
.decl exclusive(a: term, b: term)
.decl violation(v: term)
.decl child_total(derivation: term, n: int)
.decl child_sat(derivation: term, n: int)

prelude_sat_evidence: sat(A) :- evidence(A, _).
prelude_exclusive: violation(exclusive(A, B)) :- exclusive(A, B), sat(A), sat(B).
prelude_excluded_but_satisfied: violation(excluded_but_satisfied(A)) :- excluded(A), sat(A).
prelude_required_and_excluded: violation(required_and_excluded(A)) :- required(A), excluded(A).
prelude_child_total: child_total(D, count<C>) :- child(D, C).
prelude_child_sat: child_sat(D, count<C>) :- child(D, C), sat(C).
"#;

/// Predicate names owned by the prelude.
pub const RESERVED: &[&str] = &[
    "sat",
    "candidate",
    "child",
    "required",
    "excluded",
    "evidence",
    "exclusive",
    "violation",
    "child_total",
    "child_sat",
];

fn prelude() -> &'static Program {
    static P: OnceLock<Program> = OnceLock::new();
    P.get_or_init(|| {
        let mut p = Program::default();
        for item in parse_program(PRELUDE).expect("prelude parses") {
            match item {
                Item::Decl(d) => p.decls.push(d),
                Item::Rule(r) => p.rules.push(r),
                Item::Fact(f) => p.facts.push(f),
            }
        }
        p
    })
}

/// A user program expanded with the prelude and validated.
#[derive(Clone, Debug)]
pub struct Compiled {
    pub program: Program,
    pub analysis: Analysis,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub struct CompileError(pub Vec<String>);

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join("\n"))
    }
}

/// The declaration of a requirement predicate, if `name` is one.
pub fn requirement_decl<'a>(p: &'a Program, name: &str) -> Option<&'a Decl> {
    p.decl(name).filter(|d| d.has_annotation(REQUIREMENT))
}

/// Expand `@requirement` rules into core Datalog, add the prelude, and check
/// the result.
///
/// A requirement rule `name: H :- G, C1, …, Cn.` (Ci requirement atoms, G the
/// guards) becomes, with `D = name(vars of H and Ci)`:
///
/// ```text
/// name__candidate: candidate(D, H) :- G.
/// name__child1:    child(D, C1) :- G.     % one per child
/// name__sat:       sat(H) :- G, sat(C1), …, sat(Cn).
/// ```
pub fn compile(user: &Program) -> Result<Compiled, CompileError> {
    let mut errs = Vec::new();
    for d in &user.decls {
        if RESERVED.contains(&d.name.as_str()) {
            errs.push(format!(
                "predicate `{}` is reserved by the requirements layer",
                d.name
            ));
        }
    }
    let reqs: HashMap<&str, &Decl> = user
        .decls
        .iter()
        .filter(|d| d.has_annotation(REQUIREMENT))
        .map(|d| (d.name.as_str(), d))
        .collect();

    for f in &user.facts {
        check_fact(f, &reqs, &mut errs);
    }

    let mut rules = Vec::new();
    for r in &user.rules {
        if r.has_annotation(REQUIREMENT) {
            match expand(r, &reqs) {
                Ok(rs) => rules.extend(rs),
                Err(e) => errs.extend(e),
            }
            continue;
        }
        if reqs.contains_key(r.head.pred.as_str()) {
            errs.push(format!(
                "rule `{}` derives requirement `{}` but isn't tagged @{REQUIREMENT}",
                r.name, r.head.pred
            ));
        }
        for l in &r.body {
            if let Literal::Pos(a) | Literal::Neg(a) = l
                && reqs.contains_key(a.pred.as_str())
            {
                errs.push(format!(
                    "rule `{}` uses requirement `{}` directly; match `sat({})` instead",
                    r.name, a.pred, a
                ));
            }
        }
        rules.push(r.clone());
    }
    if !errs.is_empty() {
        return Err(CompileError(errs));
    }

    let pre = prelude();
    let program = Program {
        decls: user.decls.iter().chain(&pre.decls).cloned().collect(),
        rules: rules.into_iter().chain(pre.rules.iter().cloned()).collect(),
        facts: user.facts.clone(),
    };
    let analysis = analyze(&program).map_err(|e| CompileError(e.0))?;
    Ok(Compiled { program, analysis })
}

fn check_fact(f: &Fact, reqs: &HashMap<&str, &Decl>, errs: &mut Vec<String>) {
    if reqs.contains_key(f.pred.as_str()) {
        errs.push(format!(
            "fact `{f}`: `{}` is a requirement predicate; record it with `required`, \
             `excluded`, or `evidence` instead of asserting it",
            f.pred
        ));
        return;
    }
    let atoms: &[Value] = match (f.pred.as_str(), f.args.as_slice()) {
        ("required" | "excluded", [a]) => std::slice::from_ref(a),
        ("evidence", [a, _]) => std::slice::from_ref(a),
        ("exclusive", args @ [_, _]) => args,
        _ => return,
    };
    for a in atoms {
        if let Err(e) = check_requirement_value(a, reqs) {
            errs.push(format!("fact `{f}`: {e}"));
        }
    }
}

fn check_requirement_value(v: &Value, reqs: &HashMap<&str, &Decl>) -> Result<(), String> {
    let Some((name, arity)) = v.functor() else {
        return Err(format!("`{v}` is not a requirement atom"));
    };
    let Some(d) = reqs.get(name) else {
        return Err(format!("`{name}` is not a declared requirement predicate"));
    };
    if d.arity() != arity {
        return Err(format!(
            "`{name}` takes {} argument(s), got {arity}",
            d.arity()
        ));
    }
    if let Value::Compound(_, args) = v {
        for ((pname, ty), a) in d.params.iter().zip(args) {
            if !ty.admits(a) {
                return Err(format!(
                    "`{name}` argument `{pname}` expects {ty}, got `{a}`"
                ));
            }
        }
    }
    Ok(())
}

fn expand(r: &Rule, reqs: &HashMap<&str, &Decl>) -> Result<Vec<Rule>, Vec<String>> {
    let ctx = format!("requirement rule `{}`", r.name);
    let mut errs = Vec::new();
    let Some(head) = r.head.as_atom() else {
        return Err(vec![format!(
            "{ctx}: aggregates are not allowed in requirement rules"
        )]);
    };
    if !reqs.contains_key(head.pred.as_str()) {
        errs.push(format!(
            "{ctx}: head `{}` must be a requirement predicate (declared with @{REQUIREMENT})",
            head.pred
        ));
    }

    let mut children: Vec<&Atom> = Vec::new();
    let mut guards: Vec<Literal> = Vec::new();
    for l in &r.body {
        match l {
            Literal::Pos(a) if reqs.contains_key(a.pred.as_str()) => children.push(a),
            Literal::Neg(a) if reqs.contains_key(a.pred.as_str()) => errs.push(format!(
                "{ctx}: negated requirement `not {a}` is not supported; \
                 use `exclusive` or `excluded` to express exclusion"
            )),
            _ => guards.push(l.clone()),
        }
    }

    // Variables of the head and children, in order of first appearance.
    let mut vars: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for a in std::iter::once(&head).chain(children.iter().copied()) {
        let mut ordered = Vec::new();
        a.args.iter().for_each(|t| collect_ordered(t, &mut ordered));
        for v in ordered {
            if seen.insert(v.clone()) {
                vars.push(v);
            }
        }
    }
    let bound = bound_vars(&guards);
    let unbound: Vec<&str> = vars
        .iter()
        .filter(|v| !bound.contains(*v))
        .map(String::as_str)
        .collect();
    if !unbound.is_empty() {
        let shown: Vec<&str> = unbound
            .iter()
            .map(|v| if v.starts_with("_#") { "_" } else { v })
            .collect();
        errs.push(format!(
            "{ctx}: variable(s) {} must be bound by guard (non-requirement) atoms",
            shown.join(", ")
        ));
    }
    if !errs.is_empty() {
        return Err(errs);
    }

    let d = if vars.is_empty() {
        Term::Const(Value::Sym(r.name.clone()))
    } else {
        Term::Compound(r.name.clone(), vars.into_iter().map(Term::Var).collect())
    };
    let h = head.to_term();
    let mk = |name: String, pred: &str, args: Vec<Term>, body: Vec<Literal>| Rule {
        name,
        annotations: vec![],
        head: Head {
            pred: pred.into(),
            args: args.into_iter().map(HeadArg::Term).collect(),
        },
        body,
    };

    let mut out = vec![mk(
        format!("{}__candidate", r.name),
        "candidate",
        vec![d.clone(), h.clone()],
        guards.clone(),
    )];
    for (i, c) in children.iter().enumerate() {
        out.push(mk(
            format!("{}__child{}", r.name, i + 1),
            "child",
            vec![d.clone(), c.to_term()],
            guards.clone(),
        ));
    }
    let mut sat_body = guards;
    sat_body.extend(children.iter().map(|c| {
        Literal::Pos(Atom {
            pred: "sat".into(),
            args: vec![c.to_term()],
        })
    }));
    out.push(mk(format!("{}__sat", r.name), "sat", vec![h], sat_body));
    Ok(out)
}

fn collect_ordered(t: &Term, out: &mut Vec<String>) {
    match t {
        Term::Var(v) => out.push(v.clone()),
        Term::Const(_) => {}
        Term::Compound(_, args) => args.iter().for_each(|a| collect_ordered(a, out)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use datalog_core::evaluate;
    use datalog_core::parser::parse_fact;

    pub(crate) fn program(src: &str) -> Program {
        let mut p = Program::default();
        for item in parse_program(src).unwrap() {
            match item {
                Item::Decl(d) => p.decls.push(d),
                Item::Rule(r) => p.rules.push(r),
                Item::Fact(f) => p.facts.push(f),
            }
        }
        p
    }

    pub(crate) const API: &str = r#"
        .decl resource(name: symbol) @requirement
        .decl handler(method: symbol, path: string) @requirement
        .decl authenticated_requests() @requirement
        .decl session_auth() @requirement
        .decl token_auth() @requirement
        .decl collection_path(res: symbol, path: string)
        .decl item_path(res: symbol, path: string)

        @requirement resource_crud: resource(R) :- collection_path(R, P1), item_path(R, P2),
            handler(get, P1), handler(post, P1),
            handler(get, P2), handler(put, P2), handler(delete, P2).
        @requirement auth_via_session: authenticated_requests :- session_auth.
        @requirement auth_via_token: authenticated_requests :- token_auth.

        collection_path(posts, "/posts").
        item_path(posts, "/posts/{id}").
        required(resource(posts)).
        evidence(handler(put, "/posts/{id}"), "openapi.yaml").
        evidence(token_auth, "spec").
        exclusive(session_auth, token_auth).
    "#;

    #[test]
    fn expands_and_evaluates() {
        let c = compile(&program(API)).unwrap();
        let m = evaluate(&c.program, &c.analysis).unwrap();
        let has = |s: &str| m.contains(&parse_fact(s).unwrap());
        assert!(has(
            r#"candidate(resource_crud(posts, "/posts", "/posts/{id}"), resource(posts))"#
        ));
        assert!(has(
            r#"child(resource_crud(posts, "/posts", "/posts/{id}"), handler(delete, "/posts/{id}"))"#
        ));
        assert!(has(
            r#"child_total(resource_crud(posts, "/posts", "/posts/{id}"), 5)"#
        ));
        assert!(has(
            r#"child_sat(resource_crud(posts, "/posts", "/posts/{id}"), 1)"#
        ));
        assert!(!has("sat(resource(posts))"));
        assert!(has("sat(authenticated_requests)"));
        assert!(has("candidate(auth_via_session, authenticated_requests)"));
        assert!(!has("violation(exclusive(session_auth, token_auth))"));
    }

    #[test]
    fn requires_guards_to_bind_variables() {
        let e = compile(&program(
            ".decl r(x: symbol) @requirement
             .decl h(x: symbol) @requirement
             @requirement bad: r(X) :- h(X).",
        ))
        .unwrap_err();
        assert!(e.0[0].contains("must be bound by guard"), "{e}");
    }

    #[test]
    fn rejects_direct_use_of_requirements() {
        let e = compile(&program(
            ".decl r(x: symbol) @requirement .decl d(x: symbol)
             r(a).
             plain: d(X) :- r(X).",
        ))
        .unwrap_err();
        assert!(e.0.iter().any(|m| m.contains("record it with")), "{e}");
        assert!(e.0.iter().any(|m| m.contains("match `sat(r(X))`")), "{e}");
    }

    #[test]
    fn validates_stance_atoms() {
        let e = compile(&program(
            ".decl handler(method: symbol, path: string) @requirement
             required(handler(get, 5)). excluded(nope).",
        ))
        .unwrap_err();
        assert!(e.0.iter().any(|m| m.contains("expects string")), "{e}");
        assert!(
            e.0.iter().any(|m| m.contains("not a declared requirement")),
            "{e}"
        );
    }
}
