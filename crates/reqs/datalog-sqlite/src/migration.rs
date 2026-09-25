use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(M0001Initial)]
    }
}

struct M0001Initial;

impl MigrationName for M0001Initial {
    fn name(&self) -> &str {
        "m0001_initial"
    }
}

/// The schema, as SQL so it can be read at a glance.
pub const SCHEMA: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
INSERT INTO meta (key, value) VALUES ('program_generation', '0'), ('model_generation', '-1');

-- Hash-consed ground terms. Children always have smaller ids than parents.
CREATE TABLE term (
    id         INTEGER PRIMARY KEY NOT NULL,
    kind       TEXT    NOT NULL CHECK (kind IN ('int', 'str', 'sym', 'compound')),
    int_value  INTEGER,
    text_value TEXT,               -- string/symbol value, or compound functor
    arity      INTEGER NOT NULL DEFAULT 0,
    canonical  TEXT    NOT NULL UNIQUE
);
CREATE TABLE term_arg (
    term_id  INTEGER NOT NULL REFERENCES term(id),
    position INTEGER NOT NULL,
    arg_id   INTEGER NOT NULL REFERENCES term(id),
    PRIMARY KEY (term_id, position)
);
CREATE INDEX term_arg_by_arg ON term_arg(arg_id);

-- Program source. Rules and declarations are stored as canonical text.
CREATE TABLE decl (
    name     TEXT PRIMARY KEY NOT NULL,
    source   TEXT NOT NULL,
    position INTEGER NOT NULL
);
CREATE TABLE rule (
    name     TEXT PRIMARY KEY NOT NULL,
    source   TEXT NOT NULL,
    position INTEGER NOT NULL
);

-- Asserted facts; the term is the fact itself, e.g. handler(get, "/posts").
CREATE TABLE fact (
    term_id   INTEGER PRIMARY KEY NOT NULL REFERENCES term(id),
    predicate TEXT NOT NULL
);
CREATE INDEX fact_by_predicate ON fact(predicate);

-- Last computed model. iteration is NULL for asserted facts.
CREATE TABLE model_fact (
    term_id   INTEGER PRIMARY KEY NOT NULL REFERENCES term(id),
    predicate TEXT NOT NULL,
    iteration INTEGER
);
CREATE INDEX model_fact_by_predicate ON model_fact(predicate);

-- One proof per derived fact.
CREATE TABLE proof (
    term_id  INTEGER PRIMARY KEY NOT NULL REFERENCES model_fact(term_id),
    rule     TEXT NOT NULL,
    bindings TEXT NOT NULL             -- JSON: variable -> canonical term
);
CREATE TABLE proof_premise (
    term_id         INTEGER NOT NULL REFERENCES proof(term_id),
    position        INTEGER NOT NULL,
    kind            TEXT    NOT NULL CHECK (kind IN ('fact', 'absent', 'summary')),
    premise_term_id INTEGER REFERENCES term(id),
    text            TEXT,
    PRIMARY KEY (term_id, position)
);
CREATE INDEX proof_premise_by_premise ON proof_premise(premise_term_id);
"#;

#[async_trait::async_trait]
impl MigrationTrait for M0001Initial {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(SCHEMA).await?;
        Ok(())
    }
}
