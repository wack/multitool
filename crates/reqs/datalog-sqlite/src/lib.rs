//! SQLite persistence for `datalog-core`, via SeaORM.
//!
//! Rules and declarations are stored as canonical source text. Facts, model
//! facts, and proofs refer to hash-consed rows in `term`, so SQL can join on
//! them directly. See [`migration::SCHEMA`] for the tables.

pub mod entity;
pub mod migration;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use datalog_core::ast::{Fact, Program};
use datalog_core::eval::{Derivation, Model, Premise, Proof};
use datalog_core::parser::{ParseError, parse_decl, parse_rule, parse_value};
use datalog_core::store::{Change, ModelSnapshot, Store};
use datalog_core::value::Value;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, Database, DatabaseConnection, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};
use sea_orm_migration::MigratorTrait;

use entity::*;

const BATCH: usize = 500;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] sea_orm::DbErr),
    #[error("no database at {0}; run `multi reqs init` first")]
    Missing(String),
    #[error("stored {what} does not parse: {err}")]
    Parse { what: String, err: ParseError },
    #[error("corrupt store: {0}")]
    Corrupt(String),
}

pub struct SqliteStore {
    db: DatabaseConnection,
}

impl SqliteStore {
    /// Open (or with `create`, create) a database file and bring its schema
    /// up to date.
    pub async fn open(path: &Path, create: bool) -> Result<Self, StoreError> {
        if !create && !path.exists() {
            return Err(StoreError::Missing(path.display().to_string()));
        }
        let mode = if create { "rwc" } else { "rw" };
        Self::connect(&format!("sqlite://{}?mode={mode}", path.display())).await
    }

    /// Connect to any SQLite URL (e.g. `sqlite::memory:`) and migrate.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let db = Database::connect(url).await?;
        db.execute_unprepared("PRAGMA foreign_keys = ON").await?;
        migration::Migrator::up(&db, None).await?;
        Ok(SqliteStore { db })
    }

    pub fn connection(&self) -> &DatabaseConnection {
        &self.db
    }
}

async fn get_meta(c: &impl ConnectionTrait, key: &str) -> Result<i64, StoreError> {
    let row = meta::Entity::find_by_id(key.to_string())
        .one(c)
        .await?
        .ok_or_else(|| StoreError::Corrupt(format!("missing meta key {key}")))?;
    row.value
        .parse()
        .map_err(|_| StoreError::Corrupt(format!("bad meta value for {key}")))
}

async fn set_meta(c: &impl ConnectionTrait, key: &str, value: i64) -> Result<(), StoreError> {
    meta::Entity::update_many()
        .col_expr(meta::Column::Value, Expr::value(value.to_string()))
        .filter(meta::Column::Key.eq(key))
        .exec(c)
        .await?;
    Ok(())
}

/// Assigns ids to terms, reusing existing rows; `flush` writes the new ones.
struct Interner {
    ids: HashMap<String, i64>,
    next: i64,
    terms: Vec<term::ActiveModel>,
    args: Vec<term_arg::ActiveModel>,
}

impl Interner {
    async fn load(c: &impl ConnectionTrait) -> Result<Self, StoreError> {
        let rows: Vec<(i64, String)> = term::Entity::find()
            .select_only()
            .column(term::Column::Id)
            .column(term::Column::Canonical)
            .into_tuple()
            .all(c)
            .await?;
        let next = rows.iter().map(|(id, _)| *id).max().unwrap_or(0) + 1;
        Ok(Interner {
            ids: rows.into_iter().map(|(id, c)| (c, id)).collect(),
            next,
            terms: Vec::new(),
            args: Vec::new(),
        })
    }

    fn lookup(&self, v: &Value) -> Option<i64> {
        self.ids.get(&v.to_string()).copied()
    }

    fn intern(&mut self, v: &Value) -> i64 {
        let canonical = v.to_string();
        if let Some(&id) = self.ids.get(&canonical) {
            return id;
        }
        let arg_ids: Vec<i64> = match v {
            Value::Compound(_, args) => args.iter().map(|a| self.intern(a)).collect(),
            _ => vec![],
        };
        let id = self.next;
        self.next += 1;
        let (kind, int_value, text_value) = match v {
            Value::Int(n) => ("int", Some(*n), None),
            Value::Str(s) => ("str", None, Some(s.clone())),
            Value::Sym(s) => ("sym", None, Some(s.clone())),
            Value::Compound(f, _) => ("compound", None, Some(f.clone())),
        };
        self.terms.push(term::ActiveModel {
            id: Set(id),
            kind: Set(kind.into()),
            int_value: Set(int_value),
            text_value: Set(text_value),
            arity: Set(arg_ids.len() as i32),
            canonical: Set(canonical.clone()),
        });
        for (i, a) in arg_ids.into_iter().enumerate() {
            self.args.push(term_arg::ActiveModel {
                term_id: Set(id),
                position: Set(i as i32),
                arg_id: Set(a),
            });
        }
        self.ids.insert(canonical, id);
        id
    }

    async fn flush(&mut self, c: &impl ConnectionTrait) -> Result<(), StoreError> {
        for chunk in std::mem::take(&mut self.terms).chunks(BATCH) {
            term::Entity::insert_many(chunk.to_vec())
                .exec_without_returning(c)
                .await?;
        }
        for chunk in std::mem::take(&mut self.args).chunks(BATCH) {
            term_arg::Entity::insert_many(chunk.to_vec())
                .exec_without_returning(c)
                .await?;
        }
        Ok(())
    }
}

/// Every stored term as a value, by id.
async fn load_values(c: &impl ConnectionTrait) -> Result<HashMap<i64, Value>, StoreError> {
    let terms = term::Entity::find()
        .order_by_asc(term::Column::Id)
        .all(c)
        .await?;
    let mut args: HashMap<i64, Vec<(i32, i64)>> = HashMap::new();
    for a in term_arg::Entity::find().all(c).await? {
        args.entry(a.term_id)
            .or_default()
            .push((a.position, a.arg_id));
    }
    let mut out: HashMap<i64, Value> = HashMap::with_capacity(terms.len());
    let corrupt = |id: i64| StoreError::Corrupt(format!("term {id} is malformed"));
    for t in terms {
        let v = match t.kind.as_str() {
            "int" => Value::Int(t.int_value.ok_or_else(|| corrupt(t.id))?),
            "str" => Value::Str(t.text_value.ok_or_else(|| corrupt(t.id))?),
            "sym" => Value::Sym(t.text_value.ok_or_else(|| corrupt(t.id))?),
            "compound" => {
                let mut a = args.remove(&t.id).unwrap_or_default();
                a.sort();
                let vs = a
                    .into_iter()
                    .map(|(_, id)| out.get(&id).cloned().ok_or_else(|| corrupt(t.id)))
                    .collect::<Result<Vec<_>, _>>()?;
                Value::Compound(t.text_value.ok_or_else(|| corrupt(t.id))?, vs)
            }
            _ => return Err(corrupt(t.id)),
        };
        out.insert(t.id, v);
    }
    Ok(out)
}

fn fact_of(values: &HashMap<i64, Value>, id: i64) -> Result<Fact, StoreError> {
    values
        .get(&id)
        .and_then(Fact::from_value)
        .ok_or_else(|| StoreError::Corrupt(format!("term {id} is not a fact")))
}

async fn max_position(c: &impl ConnectionTrait, table: &str) -> Result<i64, StoreError> {
    let row = c
        .query_one_raw(sea_orm::Statement::from_string(
            c.get_database_backend(),
            format!("SELECT COALESCE(MAX(position), 0) AS m FROM {table}"),
        ))
        .await?
        .ok_or_else(|| StoreError::Corrupt(format!("no max position for {table}")))?;
    Ok(row.try_get::<i64>("", "m")?)
}

impl Store for SqliteStore {
    type Error = StoreError;

    async fn load_program(&self) -> Result<Program, StoreError> {
        let c = &self.db;
        let mut p = Program::default();
        for d in decl::Entity::find()
            .order_by_asc(decl::Column::Position)
            .all(c)
            .await?
        {
            p.decls
                .push(parse_decl(&d.source).map_err(|err| StoreError::Parse {
                    what: format!("declaration `{}`", d.name),
                    err,
                })?);
        }
        for r in rule::Entity::find()
            .order_by_asc(rule::Column::Position)
            .all(c)
            .await?
        {
            let mut rule = parse_rule(&r.source).map_err(|err| StoreError::Parse {
                what: format!("rule `{}`", r.name),
                err,
            })?;
            rule.name = r.name;
            p.rules.push(rule);
        }
        let values = load_values(c).await?;
        for f in fact::Entity::find().all(c).await? {
            p.facts.push(fact_of(&values, f.term_id)?);
        }
        p.facts.sort();
        Ok(p)
    }

    async fn apply(&self, changes: &[Change]) -> Result<(), StoreError> {
        let txn = self.db.begin().await?;
        let mut interner = Interner::load(&txn).await?;
        let mut changed = false;
        for ch in changes {
            match ch {
                Change::PutDecl(d) => {
                    let source = d.to_string();
                    match decl::Entity::find_by_id(d.name.clone()).one(&txn).await? {
                        Some(row) if row.source == source => {}
                        Some(_) => {
                            decl::Entity::update_many()
                                .col_expr(decl::Column::Source, Expr::value(source))
                                .filter(decl::Column::Name.eq(d.name.clone()))
                                .exec(&txn)
                                .await?;
                            changed = true;
                        }
                        None => {
                            let position = max_position(&txn, "decl").await? + 1;
                            decl::Entity::insert(decl::ActiveModel {
                                name: Set(d.name.clone()),
                                source: Set(source),
                                position: Set(position),
                            })
                            .exec_without_returning(&txn)
                            .await?;
                            changed = true;
                        }
                    }
                }
                Change::RemoveDecl(name) => {
                    let r = decl::Entity::delete_by_id(name.clone()).exec(&txn).await?;
                    changed |= r.rows_affected > 0;
                }
                Change::PutRule(r) => {
                    let source = r.to_string();
                    match rule::Entity::find_by_id(r.name.clone()).one(&txn).await? {
                        Some(row) if row.source == source => {}
                        Some(_) => {
                            rule::Entity::update_many()
                                .col_expr(rule::Column::Source, Expr::value(source))
                                .filter(rule::Column::Name.eq(r.name.clone()))
                                .exec(&txn)
                                .await?;
                            changed = true;
                        }
                        None => {
                            let position = max_position(&txn, "rule").await? + 1;
                            rule::Entity::insert(rule::ActiveModel {
                                name: Set(r.name.clone()),
                                source: Set(source),
                                position: Set(position),
                            })
                            .exec_without_returning(&txn)
                            .await?;
                            changed = true;
                        }
                    }
                }
                Change::RemoveRule(name) => {
                    let r = rule::Entity::delete_by_id(name.clone()).exec(&txn).await?;
                    changed |= r.rows_affected > 0;
                }
                Change::RenameRule { from, to } => {
                    let Some(row) = rule::Entity::find_by_id(from.clone()).one(&txn).await? else {
                        continue;
                    };
                    let mut parsed = parse_rule(&row.source).map_err(|err| StoreError::Parse {
                        what: format!("rule `{from}`"),
                        err,
                    })?;
                    parsed.name = to.clone();
                    rule::Entity::delete_by_id(from.clone()).exec(&txn).await?;
                    rule::Entity::insert(rule::ActiveModel {
                        name: Set(to.clone()),
                        source: Set(parsed.to_string()),
                        position: Set(row.position),
                    })
                    .exec_without_returning(&txn)
                    .await?;
                    changed = true;
                }
                Change::AddFact(f) => {
                    let id = interner.intern(&f.to_value());
                    interner.flush(&txn).await?;
                    if fact::Entity::find_by_id(id).one(&txn).await?.is_none() {
                        fact::Entity::insert(fact::ActiveModel {
                            term_id: Set(id),
                            predicate: Set(f.pred.clone()),
                        })
                        .exec_without_returning(&txn)
                        .await?;
                        changed = true;
                    }
                }
                Change::RemoveFact(f) => {
                    if let Some(id) = interner.lookup(&f.to_value()) {
                        let r = fact::Entity::delete_by_id(id).exec(&txn).await?;
                        changed |= r.rows_affected > 0;
                    }
                }
            }
        }
        if changed {
            let g = get_meta(&txn, "program_generation").await?;
            set_meta(&txn, "program_generation", g + 1).await?;
        }
        txn.commit().await?;
        Ok(())
    }

    async fn save_model(&self, model: &Model) -> Result<(), StoreError> {
        let txn = self.db.begin().await?;
        proof_premise::Entity::delete_many().exec(&txn).await?;
        proof::Entity::delete_many().exec(&txn).await?;
        model_fact::Entity::delete_many().exec(&txn).await?;

        let mut interner = Interner::load(&txn).await?;
        let mut facts = Vec::new();
        let mut proofs = Vec::new();
        let mut premises = Vec::new();
        for f in model.facts() {
            let id = interner.intern(&f.to_value());
            let d = model.derivation(&f);
            facts.push(model_fact::ActiveModel {
                term_id: Set(id),
                predicate: Set(f.pred.clone()),
                iteration: Set(d.map(|d| d.iteration as i64)),
            });
            let Some(d) = d else { continue };
            let bindings: BTreeMap<&str, String> = d
                .proof
                .bindings
                .iter()
                .map(|(k, v)| (k.as_str(), v.to_string()))
                .collect();
            proofs.push(proof::ActiveModel {
                term_id: Set(id),
                rule: Set(d.proof.rule.clone()),
                bindings: Set(serde_json::to_string(&bindings).expect("bindings serialize")),
            });
            for (i, p) in d.proof.premises.iter().enumerate() {
                let (kind, pid, text) = match p {
                    Premise::Fact(pf) => ("fact", Some(interner.intern(&pf.to_value())), None),
                    Premise::Absent(s) => ("absent", None, Some(s.clone())),
                    Premise::Summary(s) => ("summary", None, Some(s.clone())),
                };
                premises.push(proof_premise::ActiveModel {
                    term_id: Set(id),
                    position: Set(i as i32),
                    kind: Set(kind.into()),
                    premise_term_id: Set(pid),
                    text: Set(text),
                });
            }
        }
        interner.flush(&txn).await?;
        for chunk in facts.chunks(BATCH) {
            model_fact::Entity::insert_many(chunk.to_vec())
                .exec_without_returning(&txn)
                .await?;
        }
        for chunk in proofs.chunks(BATCH) {
            proof::Entity::insert_many(chunk.to_vec())
                .exec_without_returning(&txn)
                .await?;
        }
        for chunk in premises.chunks(BATCH) {
            proof_premise::Entity::insert_many(chunk.to_vec())
                .exec_without_returning(&txn)
                .await?;
        }

        // Drop terms nothing refers to any more.
        let live = "WITH RECURSIVE live(id) AS (
                SELECT term_id FROM fact
                UNION SELECT term_id FROM model_fact
                UNION SELECT premise_term_id FROM proof_premise WHERE premise_term_id IS NOT NULL
                UNION SELECT a.arg_id FROM term_arg a JOIN live l ON a.term_id = l.id
            )";
        txn.execute_unprepared(&format!(
            "{live} DELETE FROM term_arg WHERE term_id NOT IN (SELECT id FROM live)"
        ))
        .await?;
        txn.execute_unprepared(&format!(
            "{live} DELETE FROM term WHERE id NOT IN (SELECT id FROM live)"
        ))
        .await?;

        let g = get_meta(&txn, "program_generation").await?;
        set_meta(&txn, "model_generation", g).await?;
        txn.commit().await?;
        Ok(())
    }

    async fn load_model(&self) -> Result<Option<ModelSnapshot>, StoreError> {
        let c = &self.db;
        let model_gen = get_meta(c, "model_generation").await?;
        if model_gen < 0 {
            return Ok(None);
        }
        let program_gen = get_meta(c, "program_generation").await?;
        let values = load_values(c).await?;

        let mut model = Model::default();
        let mut derived_ids = HashMap::new();
        for mf in model_fact::Entity::find().all(c).await? {
            let f = fact_of(&values, mf.term_id)?;
            if let Some(i) = mf.iteration {
                derived_ids.insert(mf.term_id, (f.clone(), i as u32));
            }
            model.relations.entry(f.pred).or_default().insert(f.args);
        }
        let mut premises: HashMap<i64, Vec<(i32, Premise)>> = HashMap::new();
        for pp in proof_premise::Entity::find().all(c).await? {
            let p = match (pp.kind.as_str(), pp.premise_term_id, pp.text) {
                ("fact", Some(id), _) => Premise::Fact(fact_of(&values, id)?),
                ("absent", _, Some(t)) => Premise::Absent(t),
                ("summary", _, Some(t)) => Premise::Summary(t),
                _ => {
                    return Err(StoreError::Corrupt(format!(
                        "bad premise for term {}",
                        pp.term_id
                    )));
                }
            };
            premises
                .entry(pp.term_id)
                .or_default()
                .push((pp.position, p));
        }
        for pr in proof::Entity::find().all(c).await? {
            let Some((fact, iteration)) = derived_ids.get(&pr.term_id).cloned() else {
                continue;
            };
            let raw: BTreeMap<String, String> = serde_json::from_str(&pr.bindings)
                .map_err(|e| StoreError::Corrupt(format!("bad bindings: {e}")))?;
            let bindings = raw
                .into_iter()
                .map(|(k, v)| {
                    parse_value(&v)
                        .map(|v| (k, v))
                        .map_err(|err| StoreError::Parse {
                            what: "binding".into(),
                            err,
                        })
                })
                .collect::<Result<_, _>>()?;
            let mut ps = premises.remove(&pr.term_id).unwrap_or_default();
            ps.sort_by_key(|(i, _)| *i);
            model.derived.insert(
                fact,
                Derivation {
                    iteration,
                    proof: Proof {
                        rule: pr.rule,
                        bindings,
                        premises: ps.into_iter().map(|(_, p)| p).collect(),
                    },
                },
            );
        }
        Ok(Some(ModelSnapshot {
            model,
            stale: model_gen != program_gen,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datalog_core::parser::{parse_decl, parse_fact, parse_rule};
    use datalog_core::{analyze, evaluate};

    #[tokio::test]
    async fn round_trips_program_and_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        assert!(matches!(
            SqliteStore::open(&path, false).await,
            Err(StoreError::Missing(_))
        ));
        let s = SqliteStore::open(&path, true).await.unwrap();

        let mut rule =
            parse_rule("step: reach(X, Z) :- reach(X, Y), edge(Y, Z), not blocked(f(Z, _)).")
                .unwrap();
        s.apply(&[
            Change::PutDecl(parse_decl(".decl edge(a: symbol, b: symbol)").unwrap()),
            Change::PutDecl(parse_decl(".decl reach(a: symbol, b: symbol)").unwrap()),
            Change::PutDecl(parse_decl(".decl blocked(x: term)").unwrap()),
            Change::PutRule(parse_rule("base: reach(X, Y) :- edge(X, Y).").unwrap()),
            Change::PutRule(rule.clone()),
            Change::AddFact(parse_fact("edge(a, b)").unwrap()),
            Change::AddFact(parse_fact("edge(b, c)").unwrap()),
            Change::AddFact(parse_fact(r#"blocked(f(c, "x y"))"#).unwrap()),
        ])
        .await
        .unwrap();
        s.apply(&[Change::RenameRule {
            from: "step".into(),
            to: "hop".into(),
        }])
        .await
        .unwrap();
        rule.name = "hop".into();

        let p = s.load_program().await.unwrap();
        assert_eq!(p.decls.len(), 3);
        assert_eq!(p.rules[1], rule);
        assert_eq!(p.facts.len(), 3);
        assert!(s.load_model().await.unwrap().is_none());

        let m = evaluate(&p, &analyze(&p).unwrap()).unwrap();
        s.save_model(&m).await.unwrap();
        let snap = s.load_model().await.unwrap().unwrap();
        assert!(!snap.stale);
        assert_eq!(snap.model, m);

        s.apply(&[Change::RemoveFact(parse_fact("edge(b, c)").unwrap())])
            .await
            .unwrap();
        assert!(s.load_model().await.unwrap().unwrap().stale);
        assert_eq!(s.load_program().await.unwrap().facts.len(), 2);
    }
}
