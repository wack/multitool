//! SeaORM entities. The schema itself lives in [`crate::migration`].

macro_rules! no_relations {
    () => {
        #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}
        impl ActiveModelBehavior for ActiveModel {}
    };
}

pub mod meta {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "meta")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        pub value: String,
    }
    no_relations!();
}

/// Hash-consed ground terms. Facts, model facts, and proof premises refer
/// to terms by id; compound terms refer to their arguments via `term_arg`.
pub mod term {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "term")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i64,
        /// `int`, `str`, `sym`, or `compound`.
        pub kind: String,
        pub int_value: Option<i64>,
        /// String or symbol value, or the functor of a compound.
        pub text_value: Option<String>,
        pub arity: i32,
        /// Canonical source text; unique, so equal terms share one row.
        #[sea_orm(unique)]
        pub canonical: String,
    }
    no_relations!();
}

pub mod term_arg {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "term_arg")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub term_id: i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub position: i32,
        pub arg_id: i64,
    }
    no_relations!();
}

pub mod decl {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "decl")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub name: String,
        pub source: String,
        pub position: i64,
    }
    no_relations!();
}

pub mod rule {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "rule")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub name: String,
        /// Canonical single-line source, including annotations and name.
        pub source: String,
        pub position: i64,
    }
    no_relations!();
}

/// Asserted facts. The term is the fact itself viewed as a term.
pub mod fact {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "fact")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub term_id: i64,
        pub predicate: String,
    }
    no_relations!();
}

/// Every fact in the last computed model. `iteration` is null for asserted
/// facts.
pub mod model_fact {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "model_fact")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub term_id: i64,
        pub predicate: String,
        pub iteration: Option<i64>,
    }
    no_relations!();
}

pub mod proof {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "proof")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub term_id: i64,
        pub rule: String,
        /// JSON object: variable name → canonical term text.
        pub bindings: String,
    }
    no_relations!();
}

pub mod proof_premise {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "proof_premise")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub term_id: i64,
        #[sea_orm(primary_key, auto_increment = false)]
        pub position: i32,
        /// `fact`, `absent`, or `summary`.
        pub kind: String,
        pub premise_term_id: Option<i64>,
        pub text: Option<String>,
    }
    no_relations!();
}
