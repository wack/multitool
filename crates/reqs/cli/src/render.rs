use std::collections::HashSet;
use std::fmt::Write;

use datalog_core::ast::Fact;
use datalog_core::eval::{Model, Premise};
use requirements::AtomStatus;
use requirements::report::{Alternative, AtomSummary, ExplainNode, Stance, TraceNode};

/// A generic tree for box-drawing output.
pub struct Tree {
    pub line: String,
    pub children: Vec<Tree>,
}

impl Tree {
    pub fn leaf(line: impl Into<String>) -> Self {
        Tree {
            line: line.into(),
            children: vec![],
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{}", self.line);
        let n = self.children.len();
        for (i, c) in self.children.iter().enumerate() {
            c.render_into(&mut out, "", i + 1 == n);
        }
        out
    }

    fn render_into(&self, out: &mut String, prefix: &str, last: bool) {
        let _ = writeln!(
            out,
            "{prefix}{}{}",
            if last { "└─ " } else { "├─ " },
            self.line
        );
        let child_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
        let n = self.children.len();
        for (i, c) in self.children.iter().enumerate() {
            c.render_into(out, &child_prefix, i + 1 == n);
        }
    }
}

pub fn atom_line(atom: &str, status: AtomStatus, stance: Stance, evidence: &[String]) -> String {
    let mut s = format!("{atom}  [{}]", status.as_str());
    if stance != Stance::Unknown {
        let _ = write!(s, " {}", stance.as_str());
    }
    if !evidence.is_empty() {
        let _ = write!(s, "  evidence: {}", evidence.join(", "));
    }
    s
}

pub fn alt_label(a: &Alternative) -> String {
    format!(
        "{} {}/{} {}",
        a.rule,
        a.satisfied,
        a.total,
        a.status.as_str()
    )
}

fn missing(a: &Alternative) -> Option<Tree> {
    let m: Vec<&str> = a
        .children
        .iter()
        .filter(|c| !c.satisfied)
        .map(|c| c.atom.as_str())
        .collect();
    (!m.is_empty()).then(|| Tree::leaf(format!("missing: {}", m.join(", "))))
}

pub fn trace_tree(n: &TraceNode) -> Tree {
    let mut t = Tree::leaf(atom_line(&n.atom, n.status, n.stance, &n.evidence));
    if n.cycle {
        t.line.push_str("  (cycle)");
        return t;
    }
    for e in &n.parents {
        let mut parent = trace_tree(&e.parent);
        parent.line = format!("{}  via {}", parent.line, alt_label(&e.via));
        if let Some(m) = missing(&e.via) {
            parent.children.insert(0, m);
        }
        t.children.push(parent);
    }
    t
}

pub fn explain_tree(n: &ExplainNode) -> Tree {
    let mut t = Tree::leaf(atom_line(&n.atom, n.status, n.stance, &n.evidence));
    if n.cycle {
        t.line.push_str("  (cycle)");
    }
    if n.truncated {
        t.line.push_str("  (…)");
    }
    for e in &n.alternatives {
        let mut alt = Tree::leaf(format!("alternative {}", alt_label(&e.alternative)));
        alt.children = e.children.iter().map(explain_tree).collect();
        t.children.push(alt);
    }
    t
}

/// Proof tree for a fact: derived facts expand into their premises.
pub fn proof_tree(model: &Model, f: &Fact) -> Tree {
    fn go(model: &Model, f: &Fact, seen: &mut HashSet<Fact>) -> Tree {
        let Some(d) = model.derivation(f) else {
            return Tree::leaf(format!("{f}  (asserted)"));
        };
        let mut t = Tree::leaf(format!(
            "{f}  (rule {}, round {})",
            d.proof.rule, d.iteration
        ));
        if !seen.insert(f.clone()) {
            t.line.push_str("  (cycle)");
            return t;
        }
        for p in &d.proof.premises {
            t.children.push(match p {
                Premise::Fact(pf) => go(model, pf, seen),
                Premise::Absent(s) => Tree::leaf(format!("{s}  (no match)")),
                Premise::Summary(s) => Tree::leaf(s.clone()),
            });
        }
        seen.remove(f);
        t
    }
    go(model, f, &mut HashSet::new())
}

pub fn status_table(rows: &[AtomSummary]) -> String {
    let header = ["STATUS", "STANCE", "ATOM", "ALTERNATIVES"];
    let cells: Vec<[String; 4]> = rows
        .iter()
        .map(|r| {
            [
                r.status.as_str().to_string(),
                if r.stance == Stance::Unknown {
                    "-".into()
                } else {
                    r.stance.as_str().to_string()
                },
                r.atom.clone(),
                if r.alternatives.is_empty() {
                    if r.evidence.is_empty() {
                        "-".into()
                    } else {
                        format!("evidence: {}", r.evidence.join(", "))
                    }
                } else {
                    r.alternatives
                        .iter()
                        .map(alt_label)
                        .collect::<Vec<_>>()
                        .join("; ")
                },
            ]
        })
        .collect();
    let mut widths = header.map(str::len);
    for row in &cells {
        for (w, c) in widths.iter_mut().zip(row) {
            *w = (*w).max(c.chars().count());
        }
    }
    let mut out = String::new();
    let line = |out: &mut String, row: [&str; 4]| {
        let _ = writeln!(
            out,
            "{:w0$}  {:w1$}  {:w2$}  {}",
            row[0],
            row[1],
            row[2],
            row[3],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2]
        );
    };
    line(&mut out, header);
    for r in &cells {
        line(&mut out, [&r[0], &r[1], &r[2], &r[3]]);
    }
    out
}
