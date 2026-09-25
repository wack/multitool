//! The `multi reqs` subcommand: requirement AND-OR graphs, evaluated by
//! `datalog-core` and stored in SQLite by `datalog-sqlite`.
//!
//! The multi CLI embeds [`ReqsArgs`] as a subcommand and hands it to [`run`].
//! See `crates/reqs/README.md` for the language and command reference.

mod render;

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand, ValueEnum};
use datalog_core::ast::{Fact, Program};
use datalog_core::eval::Model;
use datalog_core::evaluate;
use datalog_core::parser::{
    Item, parse_atom, parse_decl, parse_fact, parse_program, parse_rule, parse_value,
};
use datalog_core::store::{Change, Store};
use datalog_core::value::{Value, is_ident};
use datalog_sqlite::SqliteStore;
use requirements::report::AtomSummary;
use requirements::{AltStatus, Report, Stance, compile};
use serde_json::json;

use render::*;

/// Arguments of `multi reqs`.
#[derive(Args, Clone)]
pub struct ReqsArgs {
    /// SQLite database file.
    #[arg(long, global = true, env = "REQS_DB", default_value = "reqs.db")]
    db: PathBuf,
    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = Format::Text)]
    format: Format,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Text,
    Json,
}

#[derive(Subcommand, Clone)]
enum Cmd {
    /// Create the database and run migrations.
    Init,
    /// Import declarations, rules, and facts from a .dl file (atomically).
    Load { file: PathBuf },
    /// Print the stored program as .dl source.
    Export {
        /// Only rules.
        #[arg(long)]
        rules: bool,
        /// Only facts.
        #[arg(long)]
        facts: bool,
    },
    /// Generic Datalog operations.
    #[command(subcommand)]
    Dl(DlCmd),
    /// Mark an atom as required (clears `excluded`).
    Require { atom: String },
    /// Mark an atom as excluded (clears `required`).
    Exclude { atom: String },
    /// Clear an atom's stance back to unknown.
    Unset { atom: String },
    /// Record where a requirement is observed to be met.
    #[command(subcommand)]
    Evidence(EvidenceCmd),
    /// Declare two atoms mutually exclusive (reported, not enforced).
    Exclusive {
        a: String,
        b: String,
        /// Remove the declaration instead.
        #[arg(long)]
        rm: bool,
    },
    /// Expand, evaluate, and store the model.
    Compute,
    /// Status of every atom the model mentions.
    Status {
        #[arg(long, value_enum)]
        only: Option<StatusFilter>,
        /// Only atoms marked required.
        #[arg(long)]
        required: bool,
    },
    /// Walk up from an atom to every root that depends on it.
    Trace { atom: String },
    /// Walk down from an atom through its alternatives and children.
    Explain {
        atom: String,
        /// Maximum depth of atoms below the root.
        #[arg(long)]
        depth: Option<usize>,
    },
    /// List constraint violations with their causes.
    Violations,
}

#[derive(Clone, Copy, ValueEnum)]
enum StatusFilter {
    Satisfied,
    Partial,
    Unsupported,
}

#[derive(Subcommand, Clone)]
enum DlCmd {
    #[command(subcommand)]
    Decl(DeclCmd),
    #[command(subcommand)]
    Rule(RuleCmd),
    #[command(subcommand)]
    Fact(FactCmd),
    /// Validate the program (after requirement expansion).
    Check,
    /// Print the compiled program: user rules with requirements expanded, plus the prelude.
    Expand,
    /// Match a pattern against the stored model, e.g. 'sat(resource(X))'.
    Query { pattern: String },
    /// Show the proof tree for a fact in the stored model.
    Why { atom: String },
}

#[derive(Subcommand, Clone)]
enum DeclCmd {
    /// e.g. '.decl handler(method: symbol, path: string) @requirement'
    Add {
        source: String,
        /// Replace an existing declaration of the same name.
        #[arg(long)]
        replace: bool,
    },
    List,
    Rm {
        name: String,
    },
}

#[derive(Subcommand, Clone)]
enum RuleCmd {
    /// e.g. multi reqs dl rule add reach 'reach(X, Y) :- edge(X, Y).'
    Add {
        name: String,
        source: String,
    },
    List {
        /// Only rules whose head is this predicate.
        #[arg(long)]
        head: Option<String>,
    },
    Show {
        name: String,
    },
    /// Replace a rule's source, keeping its name and position.
    Edit {
        name: String,
        source: String,
    },
    Rename {
        old: String,
        new: String,
    },
    Rm {
        name: String,
    },
}

#[derive(Subcommand, Clone)]
enum FactCmd {
    Add {
        atom: String,
    },
    List {
        #[arg(long)]
        predicate: Option<String>,
    },
    Rm {
        atom: String,
    },
}

#[derive(Subcommand, Clone)]
enum EvidenceCmd {
    Add {
        atom: String,
        #[arg(long)]
        source: String,
    },
    List {
        atom: Option<String>,
    },
    /// Remove evidence for an atom (all sources unless --source is given).
    Rm {
        atom: String,
        #[arg(long)]
        source: Option<String>,
    },
}

struct Ctx {
    store: SqliteStore,
    format: Format,
}

impl Ctx {
    fn emit(&self, text: impl FnOnce() -> String, json: impl FnOnce() -> serde_json::Value) {
        match self.format {
            Format::Text => {
                let mut t = text();
                if !t.is_empty() && !t.ends_with('\n') {
                    t.push('\n');
                }
                out(&t);
            }
            Format::Json => out(&format!(
                "{}\n",
                serde_json::to_string_pretty(&json()).expect("json")
            )),
        }
    }

    /// Validate the program with these changes applied, then persist them.
    async fn mutate(&self, changes: Vec<Change>, done: &str) -> Result<()> {
        let mut p = self.store.load_program().await?;
        let mut changed = false;
        for c in &changes {
            changed |= p.apply(c);
        }
        compile(&p).map_err(|e| anyhow!("change rejected:\n{e}"))?;
        self.store.apply(&changes).await?;
        let msg = if changed {
            done.to_string()
        } else {
            format!("{done} (no change)")
        };
        self.emit(
            || msg.clone(),
            || json!({ "ok": true, "changed": changed, "message": msg }),
        );
        Ok(())
    }

    async fn model(&self) -> Result<Model> {
        let snap = self
            .store
            .load_model()
            .await?
            .ok_or_else(|| anyhow!("no model yet; run `multi reqs compute`"))?;
        if snap.stale {
            eprintln!(
                "warning: the program changed since the last `multi reqs compute`; results may be out of date"
            );
        }
        Ok(snap.model)
    }
}

/// Write to stdout, exiting quietly if the reader went away (e.g. `| head`).
fn out(s: &str) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    if let Err(e) = stdout.write_all(s.as_bytes()).and_then(|_| stdout.flush()) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("error: writing output: {e}");
        std::process::exit(1);
    }
}

fn value(src: &str) -> Result<Value> {
    parse_value(src).with_context(|| format!("invalid atom `{src}`"))
}

fn fact(pred: &str, args: Vec<Value>) -> Fact {
    Fact::new(pred, args)
}

fn check_name(name: &str) -> Result<()> {
    if !is_ident(name) {
        bail!("invalid rule name `{name}` (use a lowercase identifier like `resource_crud`)");
    }
    Ok(())
}

/// Execute a `multi reqs` command.
pub async fn run(cli: ReqsArgs) -> Result<()> {
    if let Cmd::Init = cli.cmd {
        SqliteStore::open(&cli.db, true).await?;
        out(&format!("initialized {}\n", cli.db.display()));
        return Ok(());
    }
    let ctx = Ctx {
        store: SqliteStore::open(&cli.db, false).await?,
        format: cli.format,
    };

    match cli.cmd {
        Cmd::Init => unreachable!(),
        Cmd::Load { file } => load(&ctx, &file).await,
        Cmd::Export { rules, facts } => {
            let mut p = ctx.store.load_program().await?;
            if rules || facts {
                if !rules {
                    p.rules.clear();
                }
                if !facts {
                    p.facts.clear();
                }
                p.decls.clear();
            }
            out(&p.to_source());
            Ok(())
        }
        Cmd::Dl(cmd) => dl(&ctx, cmd).await,
        Cmd::Require { atom } => {
            let a = value(&atom)?;
            ctx.mutate(
                vec![
                    Change::RemoveFact(fact("excluded", vec![a.clone()])),
                    Change::AddFact(fact("required", vec![a])),
                ],
                &format!("{atom}: required"),
            )
            .await
        }
        Cmd::Exclude { atom } => {
            let a = value(&atom)?;
            ctx.mutate(
                vec![
                    Change::RemoveFact(fact("required", vec![a.clone()])),
                    Change::AddFact(fact("excluded", vec![a])),
                ],
                &format!("{atom}: excluded"),
            )
            .await
        }
        Cmd::Unset { atom } => {
            let a = value(&atom)?;
            ctx.mutate(
                vec![
                    Change::RemoveFact(fact("required", vec![a.clone()])),
                    Change::RemoveFact(fact("excluded", vec![a])),
                ],
                &format!("{atom}: unknown"),
            )
            .await
        }
        Cmd::Evidence(cmd) => evidence(&ctx, cmd).await,
        Cmd::Exclusive { a, b, rm } => {
            let f = fact("exclusive", vec![value(&a)?, value(&b)?]);
            let (c, msg) = if rm {
                (Change::RemoveFact(f.clone()), format!("removed {f}"))
            } else {
                (Change::AddFact(f.clone()), format!("added {f}"))
            };
            ctx.mutate(vec![c], &msg).await
        }
        Cmd::Compute => {
            let p = ctx.store.load_program().await?;
            let c = compile(&p).map_err(|e| anyhow!("{e}"))?;
            let m = evaluate(&c.program, &c.analysis)?;
            ctx.store.save_model(&m).await?;
            let total: usize = m.relations.values().map(|r| r.len()).sum();
            let rounds = m.derived.values().map(|d| d.iteration).max().unwrap_or(0);
            let violations = m.tuples("violation").count();
            let sat = m.tuples("sat").count();
            ctx.emit(
                || {
                    let mut s = format!(
                        "computed {total} facts ({} derived, {sat} satisfied atoms) in {rounds} rounds",
                        m.derived.len()
                    );
                    if violations > 0 {
                        s.push_str(&format!("\n{violations} violation(s); see `multi reqs violations`"));
                    }
                    s
                },
                || json!({ "facts": total, "derived": m.derived.len(), "satisfied": sat, "rounds": rounds, "violations": violations }),
            );
            Ok(())
        }
        Cmd::Status { only, required } => {
            let m = ctx.model().await?;
            let r = Report::new(&m);
            let rows: Vec<AtomSummary> = r
                .atoms()
                .iter()
                .map(|a| r.summary(a))
                .filter(|s| !required || matches!(s.stance, Stance::Required | Stance::Conflicting))
                .filter(|s| match only {
                    None => true,
                    Some(StatusFilter::Satisfied) => s.status == AltStatus::Satisfied,
                    Some(StatusFilter::Partial) => s.status == AltStatus::Partial,
                    Some(StatusFilter::Unsupported) => s.status == AltStatus::Unsupported,
                })
                .collect();
            ctx.emit(
                || {
                    if rows.is_empty() {
                        "no matching atoms".into()
                    } else {
                        status_table(&rows)
                    }
                },
                || json!(rows),
            );
            Ok(())
        }
        Cmd::Trace { atom } => {
            let a = value(&atom)?;
            let m = ctx.model().await?;
            let t = Report::new(&m).trace(&a);
            ctx.emit(|| trace_tree(&t).render(), || json!(t));
            Ok(())
        }
        Cmd::Explain { atom, depth } => {
            let a = value(&atom)?;
            let m = ctx.model().await?;
            let t = Report::new(&m).explain(&a, depth);
            ctx.emit(|| explain_tree(&t).render(), || json!(t));
            Ok(())
        }
        Cmd::Violations => {
            let m = ctx.model().await?;
            let vs = Report::new(&m).violations();
            ctx.emit(
                || {
                    if vs.is_empty() {
                        return "no violations".into();
                    }
                    vs.iter()
                        .map(|v| {
                            let mut t = Tree::leaf(match &v.rule {
                                Some(r) => format!("{}  (rule {r})", v.violation),
                                None => v.violation.clone(),
                            });
                            t.children = v.because.iter().map(Tree::leaf).collect();
                            t.render()
                        })
                        .collect::<Vec<_>>()
                        .join("")
                },
                || json!(vs),
            );
            Ok(())
        }
    }
}

async fn load(ctx: &Ctx, file: &PathBuf) -> Result<()> {
    let src =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let items = parse_program(&src).map_err(|e| anyhow!("{}:{e}", file.display()))?;
    let existing = ctx.store.load_program().await?;
    let mut names: HashSet<String> = existing.rules.iter().map(|r| r.name.clone()).collect();
    let (mut d, mut r, mut f) = (0, 0, 0);
    let mut changes = Vec::new();
    for item in items {
        match item {
            Item::Decl(x) => {
                d += 1;
                changes.push(Change::PutDecl(x));
            }
            Item::Rule(mut x) => {
                r += 1;
                if x.name.is_empty() {
                    x.name = (1..)
                        .map(|i| format!("{}_{i}", x.head.pred))
                        .find(|n| !names.contains(n))
                        .expect("some name is free");
                }
                names.insert(x.name.clone());
                changes.push(Change::PutRule(x));
            }
            Item::Fact(x) => {
                f += 1;
                changes.push(Change::AddFact(x));
            }
        }
    }
    ctx.mutate(
        changes,
        &format!(
            "loaded {d} declaration(s), {r} rule(s), {f} fact(s) from {}",
            file.display()
        ),
    )
    .await
}

async fn dl(ctx: &Ctx, cmd: DlCmd) -> Result<()> {
    match cmd {
        DlCmd::Decl(DeclCmd::Add { source, replace }) => {
            let d = parse_decl(&source).map_err(|e| anyhow!("{e}"))?;
            let p = ctx.store.load_program().await?;
            if p.decl(&d.name).is_some() && !replace {
                bail!(
                    "`{}` is already declared; pass --replace to change it",
                    d.name
                );
            }
            let msg = format!("declared {}", d.name);
            ctx.mutate(vec![Change::PutDecl(d)], &msg).await
        }
        DlCmd::Decl(DeclCmd::List) => {
            let p = ctx.store.load_program().await?;
            ctx.emit(
                || p.decls.iter().map(|d| format!("{d}\n")).collect(),
                || json!(p.decls.iter().map(|d| d.to_string()).collect::<Vec<_>>()),
            );
            Ok(())
        }
        DlCmd::Decl(DeclCmd::Rm { name }) => {
            let p = ctx.store.load_program().await?;
            if p.decl(&name).is_none() {
                bail!("no declaration named `{name}`");
            }
            ctx.mutate(
                vec![Change::RemoveDecl(name.clone())],
                &format!("removed declaration {name}"),
            )
            .await
        }
        DlCmd::Rule(RuleCmd::Add { name, source }) => {
            check_name(&name)?;
            let p = ctx.store.load_program().await?;
            if p.rule(&name).is_some() {
                bail!("rule `{name}` already exists; use `multi reqs dl rule edit`");
            }
            let r = named_rule(&name, &source)?;
            ctx.mutate(vec![Change::PutRule(r)], &format!("added rule {name}"))
                .await
        }
        DlCmd::Rule(RuleCmd::Edit { name, source }) => {
            let p = ctx.store.load_program().await?;
            if p.rule(&name).is_none() {
                bail!("no rule named `{name}`");
            }
            let r = named_rule(&name, &source)?;
            ctx.mutate(vec![Change::PutRule(r)], &format!("updated rule {name}"))
                .await
        }
        DlCmd::Rule(RuleCmd::List { head }) => {
            let p = ctx.store.load_program().await?;
            let rules: Vec<_> = p
                .rules
                .iter()
                .filter(|r| head.as_ref().is_none_or(|h| &r.head.pred == h))
                .collect();
            ctx.emit(
                || rules.iter().map(|r| format!("{r}\n")).collect(),
                || {
                    json!(
                        rules
                            .iter()
                            .map(|r| json!({ "name": r.name, "source": r.to_string() }))
                            .collect::<Vec<_>>()
                    )
                },
            );
            Ok(())
        }
        DlCmd::Rule(RuleCmd::Show { name }) => {
            let p = ctx.store.load_program().await?;
            let r = p
                .rule(&name)
                .ok_or_else(|| anyhow!("no rule named `{name}`"))?;
            ctx.emit(
                || r.pretty(),
                || json!({ "name": r.name, "source": r.to_string() }),
            );
            Ok(())
        }
        DlCmd::Rule(RuleCmd::Rename { old, new }) => {
            check_name(&new)?;
            let p = ctx.store.load_program().await?;
            if p.rule(&old).is_none() {
                bail!("no rule named `{old}`");
            }
            if p.rule(&new).is_some() {
                bail!("rule `{new}` already exists");
            }
            ctx.mutate(
                vec![Change::RenameRule {
                    from: old.clone(),
                    to: new.clone(),
                }],
                &format!("renamed {old} to {new}"),
            )
            .await
        }
        DlCmd::Rule(RuleCmd::Rm { name }) => {
            let p = ctx.store.load_program().await?;
            if p.rule(&name).is_none() {
                bail!("no rule named `{name}`");
            }
            ctx.mutate(
                vec![Change::RemoveRule(name.clone())],
                &format!("removed rule {name}"),
            )
            .await
        }
        DlCmd::Fact(FactCmd::Add { atom }) => {
            let f = parse_fact(&atom).map_err(|e| anyhow!("{e}"))?;
            ctx.mutate(vec![Change::AddFact(f.clone())], &format!("added {f}"))
                .await
        }
        DlCmd::Fact(FactCmd::Rm { atom }) => {
            let f = parse_fact(&atom).map_err(|e| anyhow!("{e}"))?;
            ctx.mutate(vec![Change::RemoveFact(f.clone())], &format!("removed {f}"))
                .await
        }
        DlCmd::Fact(FactCmd::List { predicate }) => {
            let p = ctx.store.load_program().await?;
            let facts: Vec<&Fact> = p
                .facts
                .iter()
                .filter(|f| predicate.as_ref().is_none_or(|x| &f.pred == x))
                .collect();
            ctx.emit(
                || facts.iter().map(|f| format!("{f}.\n")).collect(),
                || json!(facts.iter().map(|f| f.to_string()).collect::<Vec<_>>()),
            );
            Ok(())
        }
        DlCmd::Check => {
            let p = ctx.store.load_program().await?;
            let c = compile(&p).map_err(|e| anyhow!("{e}"))?;
            let recursive = c.analysis.strata.iter().filter(|s| s.recursive).count();
            let evaluated = c
                .analysis
                .strata
                .iter()
                .filter(|s| !s.rules.is_empty())
                .count();
            ctx.emit(
                || {
                    format!(
                        "ok: {} declaration(s), {} rule(s) ({} after expansion), {} fact(s); \
                         {evaluated} evaluation stratum/strata, {recursive} recursive",
                        p.decls.len(),
                        p.rules.len(),
                        c.program.rules.len(),
                        p.facts.len()
                    )
                },
                || {
                    json!({
                        "ok": true,
                        "strata": c.analysis.strata.iter().filter(|s| !s.rules.is_empty()).map(|s| json!({
                            "predicates": s.preds,
                            "recursive": s.recursive,
                            "rules": s.rules.iter().map(|&i| &c.program.rules[i].name).collect::<Vec<_>>(),
                        })).collect::<Vec<_>>()
                    })
                },
            );
            Ok(())
        }
        DlCmd::Expand => {
            let p = ctx.store.load_program().await?;
            let c = compile(&p).map_err(|e| anyhow!("{e}"))?;
            let rules_only = Program {
                decls: c.program.decls.clone(),
                rules: c.program.rules.clone(),
                facts: vec![],
            };
            ctx.emit(
                || rules_only.to_source(),
                || {
                    json!(
                        c.program
                            .rules
                            .iter()
                            .map(|r| r.to_string())
                            .collect::<Vec<_>>()
                    )
                },
            );
            Ok(())
        }
        DlCmd::Query { pattern } => {
            let pat = parse_atom(&pattern).map_err(|e| anyhow!("{e}"))?;
            let m = ctx.model().await?;
            let rows = m.query(&pat);
            ctx.emit(
                || {
                    if rows.is_empty() {
                        "no matches".into()
                    } else {
                        rows.iter().map(|(f, _)| format!("{f}\n")).collect()
                    }
                },
                || {
                    json!(rows
                        .iter()
                        .map(|(f, b)| json!({
                            "fact": f.to_string(),
                            "bindings": b.iter().map(|(k, v)| (k.clone(), json!(v.to_string()))).collect::<serde_json::Map<_, _>>(),
                        }))
                        .collect::<Vec<_>>())
                },
            );
            Ok(())
        }
        DlCmd::Why { atom } => {
            let f = parse_fact(&atom).map_err(|e| anyhow!("{e}"))?;
            let m = ctx.model().await?;
            if !m.contains(&f) {
                bail!("`{f}` is not in the model");
            }
            let t = proof_tree(&m, &f);
            let text = t.render();
            ctx.emit(
                || text.clone(),
                || json!({ "fact": f.to_string(), "proof": text }),
            );
            Ok(())
        }
    }
}

fn named_rule(name: &str, source: &str) -> Result<datalog_core::ast::Rule> {
    let mut r = parse_rule(source).map_err(|e| anyhow!("{e}"))?;
    if !r.name.is_empty() && r.name != name {
        bail!(
            "source names the rule `{}` but the command names it `{name}`",
            r.name
        );
    }
    r.name = name.to_string();
    Ok(r)
}

async fn evidence(ctx: &Ctx, cmd: EvidenceCmd) -> Result<()> {
    match cmd {
        EvidenceCmd::Add { atom, source } => {
            let f = fact("evidence", vec![value(&atom)?, Value::Str(source)]);
            ctx.mutate(vec![Change::AddFact(f.clone())], &format!("added {f}"))
                .await
        }
        EvidenceCmd::List { atom } => {
            let a = atom.as_deref().map(value).transpose()?;
            let p = ctx.store.load_program().await?;
            let rows: Vec<&Fact> = p
                .facts
                .iter()
                .filter(|f| {
                    f.pred == "evidence" && a.as_ref().is_none_or(|a| f.args.first() == Some(a))
                })
                .collect();
            ctx.emit(
                || {
                    rows.iter()
                        .map(|f| format!("{}  {}\n", f.args[0], f.args[1]))
                        .collect()
                },
                || {
                    json!(rows
                        .iter()
                        .map(|f| json!({ "atom": f.args[0].to_string(), "source": match &f.args[1] {
                            Value::Str(s) => s.clone(), v => v.to_string() } }))
                        .collect::<Vec<_>>())
                },
            );
            Ok(())
        }
        EvidenceCmd::Rm { atom, source } => {
            let a = value(&atom)?;
            let p = ctx.store.load_program().await?;
            let changes: Vec<Change> = p
                .facts
                .iter()
                .filter(|f| {
                    f.pred == "evidence"
                        && f.args.first() == Some(&a)
                        && source
                            .as_ref()
                            .is_none_or(|s| f.args.get(1) == Some(&Value::Str(s.clone())))
                })
                .cloned()
                .map(Change::RemoveFact)
                .collect();
            if changes.is_empty() {
                bail!("no matching evidence for `{a}`");
            }
            let n = changes.len();
            ctx.mutate(changes, &format!("removed {n} evidence fact(s) for {a}"))
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    /// `ReqsArgs` is only ever parsed nested inside the multi CLI; wrap it the
    /// same way so clap's checks see a complete command.
    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        args: ReqsArgs,
    }

    #[test]
    fn args_definition_is_valid() {
        Harness::command().debug_assert();
    }

    #[test]
    fn global_flags_follow_the_subcommand() {
        let h = Harness::try_parse_from([
            "reqs",
            "status",
            "--db",
            "api.db",
            "--format",
            "json",
            "--required",
        ])
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(h.args.db, PathBuf::from("api.db"));
        assert!(h.args.format == Format::Json);
        assert!(matches!(
            h.args.cmd,
            Cmd::Status {
                only: None,
                required: true
            }
        ));
    }

    #[test]
    fn rule_names_must_be_lowercase_identifiers() {
        assert!(check_name("resource_crud").is_ok());
        assert!(check_name("ResourceCrud").is_err());
        assert!(check_name("resource-crud").is_err());
    }

    #[test]
    fn named_rule_takes_the_command_name() {
        let r = named_rule("reach", "reach(X, Y) :- edge(X, Y).").unwrap();
        assert_eq!(r.name, "reach");
        assert!(named_rule("reach", "base: reach(X, Y) :- edge(X, Y).").is_err());
    }

    #[test]
    fn tree_renders_with_box_drawing() {
        let mut root = Tree::leaf("a");
        let mut b = Tree::leaf("b");
        b.children.push(Tree::leaf("c"));
        root.children = vec![b, Tree::leaf("d")];
        assert_eq!(root.render(), "a\n├─ b\n│  └─ c\n└─ d\n");
    }
}
