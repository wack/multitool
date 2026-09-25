use std::collections::{BTreeSet, HashMap, HashSet};

use datalog_core::ast::Fact;
use datalog_core::eval::{Model, Premise};
use datalog_core::value::Value;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AltStatus {
    Satisfied,
    Partial,
    Unsupported,
}

/// An atom's status: satisfied if `sat(atom)` holds; partial if some
/// alternative has some satisfied children; unsupported otherwise.
pub type AtomStatus = AltStatus;

impl AltStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AltStatus::Satisfied => "satisfied",
            AltStatus::Partial => "partial",
            AltStatus::Unsupported => "unsupported",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stance {
    Required,
    Excluded,
    Unknown,
    /// Both required and excluded (also reported as a violation).
    Conflicting,
}

impl Stance {
    pub fn as_str(self) -> &'static str {
        match self {
            Stance::Required => "required",
            Stance::Excluded => "excluded",
            Stance::Unknown => "unknown",
            Stance::Conflicting => "conflicting",
        }
    }
}

/// One AND-set for an atom.
#[derive(Clone, Debug, Serialize)]
pub struct Alternative {
    pub derivation: String,
    pub rule: String,
    pub status: AltStatus,
    pub satisfied: i64,
    pub total: i64,
    pub children: Vec<ChildRef>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChildRef {
    pub atom: String,
    pub satisfied: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AtomSummary {
    pub atom: String,
    pub status: AtomStatus,
    pub stance: Stance,
    pub evidence: Vec<String>,
    pub alternatives: Vec<Alternative>,
}

/// Upward tree: an atom and the alternatives it participates in.
#[derive(Clone, Debug, Serialize)]
pub struct TraceNode {
    pub atom: String,
    pub status: AtomStatus,
    pub stance: Stance,
    pub evidence: Vec<String>,
    pub parents: Vec<TraceEdge>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cycle: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct TraceEdge {
    /// The parent's alternative that lists the child.
    pub via: Alternative,
    pub parent: TraceNode,
}

/// Downward tree: an atom, its alternatives, and their children.
#[derive(Clone, Debug, Serialize)]
pub struct ExplainNode {
    pub atom: String,
    pub status: AtomStatus,
    pub stance: Stance,
    pub evidence: Vec<String>,
    pub alternatives: Vec<ExplainEdge>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cycle: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExplainEdge {
    pub alternative: Alternative,
    pub children: Vec<ExplainNode>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Violation {
    pub violation: String,
    pub rule: Option<String>,
    pub because: Vec<String>,
}

/// Read-only views over a computed model.
pub struct Report<'m> {
    model: &'m Model,
    alts_by_head: HashMap<Value, Vec<Value>>,
    children_by_d: HashMap<Value, Vec<Value>>,
    ds_by_child: HashMap<Value, Vec<Value>>,
    head_by_d: HashMap<Value, Value>,
    sat: HashSet<Value>,
    required: HashSet<Value>,
    excluded: HashSet<Value>,
    evidence: HashMap<Value, Vec<String>>,
    counts: HashMap<Value, (i64, i64)>,
}

fn pairs<'a>(m: &'a Model, pred: &str) -> impl Iterator<Item = (&'a Value, &'a Value)> + 'a {
    m.tuples(pred).filter_map(|t| match t.as_slice() {
        [a, b] => Some((a, b)),
        _ => None,
    })
}

fn singles<'a>(m: &'a Model, pred: &str) -> impl Iterator<Item = &'a Value> + 'a {
    m.tuples(pred).filter_map(|t| t.first())
}

impl<'m> Report<'m> {
    pub fn new(model: &'m Model) -> Self {
        let mut r = Report {
            model,
            alts_by_head: HashMap::new(),
            children_by_d: HashMap::new(),
            ds_by_child: HashMap::new(),
            head_by_d: HashMap::new(),
            sat: singles(model, "sat").cloned().collect(),
            required: singles(model, "required").cloned().collect(),
            excluded: singles(model, "excluded").cloned().collect(),
            evidence: HashMap::new(),
            counts: HashMap::new(),
        };
        for (d, h) in pairs(model, "candidate") {
            r.alts_by_head.entry(h.clone()).or_default().push(d.clone());
            r.head_by_d.insert(d.clone(), h.clone());
        }
        for (d, c) in pairs(model, "child") {
            r.children_by_d
                .entry(d.clone())
                .or_default()
                .push(c.clone());
            r.ds_by_child.entry(c.clone()).or_default().push(d.clone());
        }
        for (a, src) in pairs(model, "evidence") {
            if let Value::Str(src) = src {
                r.evidence.entry(a.clone()).or_default().push(src.clone());
            }
        }
        for (d, n) in pairs(model, "child_total") {
            if let Value::Int(n) = n {
                r.counts.entry(d.clone()).or_default().1 = *n;
            }
        }
        for (d, n) in pairs(model, "child_sat") {
            if let Value::Int(n) = n {
                r.counts.entry(d.clone()).or_default().0 = *n;
            }
        }
        // Children in the order the rule lists them (from `name__childN`).
        for (d, cs) in r.children_by_d.iter_mut() {
            cs.sort_by_key(|c| {
                let idx = model
                    .derivation(&Fact::new("child", vec![d.clone(), c.clone()]))
                    .and_then(|dv| {
                        dv.proof
                            .rule
                            .rsplit_once("__child")
                            .and_then(|(_, n)| n.parse::<usize>().ok())
                    });
                (idx.unwrap_or(usize::MAX), c.clone())
            });
        }
        r.alts_by_head.values_mut().for_each(|v| v.sort());
        r.ds_by_child.values_mut().for_each(|v| v.sort());
        r.evidence.values_mut().for_each(|v| v.sort());
        r
    }

    pub fn is_sat(&self, a: &Value) -> bool {
        self.sat.contains(a)
    }

    pub fn stance(&self, a: &Value) -> Stance {
        match (self.required.contains(a), self.excluded.contains(a)) {
            (true, true) => Stance::Conflicting,
            (true, false) => Stance::Required,
            (false, true) => Stance::Excluded,
            (false, false) => Stance::Unknown,
        }
    }

    pub fn evidence(&self, a: &Value) -> Vec<String> {
        self.evidence.get(a).cloned().unwrap_or_default()
    }

    pub fn alternative(&self, d: &Value) -> Alternative {
        let children = self.children_by_d.get(d).cloned().unwrap_or_default();
        let (satisfied, total) = self.counts.get(d).copied().unwrap_or((0, 0));
        let status = if total == 0 || satisfied == total {
            AltStatus::Satisfied
        } else if satisfied == 0 {
            AltStatus::Unsupported
        } else {
            AltStatus::Partial
        };
        let rule = d
            .functor()
            .map(|(f, _)| f.to_string())
            .unwrap_or_else(|| d.to_string());
        Alternative {
            derivation: d.to_string(),
            rule,
            status,
            satisfied,
            total,
            children: children
                .iter()
                .map(|c| ChildRef {
                    atom: c.to_string(),
                    satisfied: self.is_sat(c),
                })
                .collect(),
        }
    }

    pub fn alternatives(&self, a: &Value) -> Vec<Alternative> {
        self.alts_by_head
            .get(a)
            .into_iter()
            .flatten()
            .map(|d| self.alternative(d))
            .collect()
    }

    pub fn status(&self, a: &Value) -> AtomStatus {
        if self.is_sat(a) {
            AltStatus::Satisfied
        } else if self.alternatives(a).iter().any(|x| x.satisfied > 0) {
            AltStatus::Partial
        } else {
            AltStatus::Unsupported
        }
    }

    pub fn summary(&self, a: &Value) -> AtomSummary {
        AtomSummary {
            atom: a.to_string(),
            status: self.status(a),
            stance: self.stance(a),
            evidence: self.evidence(a),
            alternatives: self.alternatives(a),
        }
    }

    /// Every atom the model mentions: alternative heads, children, stances,
    /// and evidence. Sorted.
    pub fn atoms(&self) -> Vec<Value> {
        let mut s: BTreeSet<Value> = BTreeSet::new();
        s.extend(self.alts_by_head.keys().cloned());
        s.extend(self.ds_by_child.keys().cloned());
        s.extend(self.required.iter().cloned());
        s.extend(self.excluded.iter().cloned());
        s.extend(self.evidence.keys().cloned());
        s.into_iter().collect()
    }

    /// From an atom up to every root, through each alternative that lists it.
    pub fn trace(&self, a: &Value) -> TraceNode {
        self.trace_inner(a, &mut Vec::new())
    }

    fn trace_inner(&self, a: &Value, path: &mut Vec<Value>) -> TraceNode {
        let cycle = path.contains(a);
        let mut node = TraceNode {
            atom: a.to_string(),
            status: self.status(a),
            stance: self.stance(a),
            evidence: self.evidence(a),
            parents: vec![],
            cycle,
        };
        if cycle {
            return node;
        }
        path.push(a.clone());
        for d in self.ds_by_child.get(a).into_iter().flatten() {
            if let Some(h) = self.head_by_d.get(d) {
                node.parents.push(TraceEdge {
                    via: self.alternative(d),
                    parent: self.trace_inner(h, path),
                });
            }
        }
        path.pop();
        node
    }

    /// From an atom down through its alternatives and their children.
    pub fn explain(&self, a: &Value, max_depth: Option<usize>) -> ExplainNode {
        self.explain_inner(a, &mut Vec::new(), max_depth)
    }

    fn explain_inner(&self, a: &Value, path: &mut Vec<Value>, depth: Option<usize>) -> ExplainNode {
        let cycle = path.contains(a);
        let alts = self.alts_by_head.get(a).cloned().unwrap_or_default();
        let truncated = depth == Some(0) && !alts.is_empty();
        let mut node = ExplainNode {
            atom: a.to_string(),
            status: self.status(a),
            stance: self.stance(a),
            evidence: self.evidence(a),
            alternatives: vec![],
            cycle,
            truncated,
        };
        if cycle || truncated {
            return node;
        }
        path.push(a.clone());
        for d in &alts {
            let children = self
                .children_by_d
                .get(d)
                .into_iter()
                .flatten()
                .map(|c| self.explain_inner(c, path, depth.map(|n| n - 1)))
                .collect();
            node.alternatives.push(ExplainEdge {
                alternative: self.alternative(d),
                children,
            });
        }
        path.pop();
        node
    }

    pub fn violations(&self) -> Vec<Violation> {
        let mut vs: Vec<&Value> = singles(self.model, "violation").collect();
        vs.sort();
        vs.into_iter()
            .map(|v| {
                let d = self
                    .model
                    .derivation(&Fact::new("violation", vec![v.clone()]));
                Violation {
                    violation: v.to_string(),
                    rule: d.map(|d| d.proof.rule.clone()),
                    because: d
                        .map(|d| {
                            d.proof
                                .premises
                                .iter()
                                .map(|p| match p {
                                    Premise::Fact(f) => f.to_string(),
                                    Premise::Absent(s) | Premise::Summary(s) => s.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile;
    use crate::compile::tests::{API, program};
    use datalog_core::evaluate;
    use datalog_core::parser::parse_value;

    fn model(src: &str) -> Model {
        let c = compile(&program(src)).unwrap();
        evaluate(&c.program, &c.analysis).unwrap()
    }

    #[test]
    fn traces_upward() {
        let m = model(API);
        let r = Report::new(&m);
        let t = r.trace(&parse_value(r#"handler(put, "/posts/{id}")"#).unwrap());
        assert_eq!(t.status, AltStatus::Satisfied);
        assert_eq!(t.evidence, vec!["openapi.yaml"]);
        assert_eq!(t.parents.len(), 1);
        let e = &t.parents[0];
        assert_eq!(e.parent.atom, "resource(posts)");
        assert_eq!(e.parent.stance, Stance::Required);
        assert_eq!(e.via.status, AltStatus::Partial);
        assert_eq!((e.via.satisfied, e.via.total), (1, 5));
        assert_eq!(e.via.children[0].atom, r#"handler(get, "/posts")"#);
    }

    #[test]
    fn explains_or_alternatives() {
        let m = model(API);
        let r = Report::new(&m);
        let x = r.explain(&parse_value("authenticated_requests").unwrap(), None);
        assert_eq!(x.status, AltStatus::Satisfied);
        let alts: Vec<(&str, AltStatus)> = x
            .alternatives
            .iter()
            .map(|e| (e.alternative.rule.as_str(), e.alternative.status))
            .collect();
        assert_eq!(
            alts,
            vec![
                ("auth_via_session", AltStatus::Unsupported),
                ("auth_via_token", AltStatus::Satisfied)
            ]
        );
    }

    #[test]
    fn reports_violations() {
        let m = model(&format!(
            "{API}\nevidence(session_auth, \"legacy\").\nexcluded(token_auth)."
        ));
        let r = Report::new(&m);
        let v: Vec<String> = r.violations().into_iter().map(|v| v.violation).collect();
        assert_eq!(
            v,
            vec![
                "excluded_but_satisfied(token_auth)",
                "exclusive(session_auth, token_auth)"
            ]
        );
    }
}
