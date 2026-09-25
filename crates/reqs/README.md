# reqs

Requirements as AND-OR graphs, evaluated by a small Datalog engine and stored in SQLite.

`reqs` ships inside the multi CLI as the `multi reqs` subcommand: every command below is run as `multi reqs <command>`.

A requirement rule says "this atom is met when all of these child atoms are met" (AND). Several rules for the same atom are alternatives (OR). You record *evidence* for the leaves, mark atoms *required* or *excluded*, and ask which requirements are satisfied, which are partially supported, what's missing, and why.

## Workspace

The crates live under `crates/reqs/` and are members of the multitool workspace.

| Crate | What it is |
|---|---|
| `datalog-core` | Parser, static checks, semi-naive evaluator with provenance, and the `Store` trait. No I/O. |
| `requirements` | Compiles `@requirement` rules into core Datalog, adds a prelude, and builds reports (status, trace, explain, violations). |
| `datalog-sqlite` | `Store` implementation on SQLite via SeaORM 2.0, with hash-consed terms. |
| `cli` (`reqs-cli`) | The `multi reqs` subcommand (clap): `ReqsArgs` and `run`, dispatched from `src/cmd/reqs.rs` in the multi CLI. |

## Quick start

```sh
cargo build --release
export REQS_DB=api.db            # or pass --db
multi reqs init
multi reqs load crates/reqs/examples/api.dl
multi reqs compute
multi reqs status --required
multi reqs trace 'handler(put, "/posts/{id}")'
multi reqs explain api_ready --depth 2
```

`trace` walks up from an atom to everything that depends on it:

```
handler(put, "/posts/{id}")  [satisfied]  evidence: openapi.yaml
└─ resource(posts)  [partial]  via resource_crud 3/5 partial
   ├─ missing: handler(get, "/posts/{id}"), handler(delete, "/posts/{id}")
   └─ api_ready  [partial] required  via ready 1/3 partial
      └─ missing: resource(posts), resource(users)
```

`explain` walks down through alternatives:

```
authenticated_requests  [satisfied]
├─ alternative auth_via_session 0/1 unsupported
│  └─ session_auth  [unsupported]
└─ alternative auth_via_token 1/1 satisfied
   └─ token_auth  [satisfied]  evidence: src/auth/jwt.rs
```

Every report command also takes `--format json`.

## The language

```prolog
% Declarations: every predicate is declared. Types: int, string, symbol, term.
.decl edge(a: symbol, b: symbol)
.decl path(a: symbol, b: symbol)
.decl n_out(a: symbol, n: int)

% Facts must be ground.
edge(a, b).  edge(b, c).

% Rules are named (unnamed rules get `<head>_<n>` on load).
base: path(X, Y) :- edge(X, Y).
step: path(X, Z) :- path(X, Y), edge(Y, Z).

% Negation (stratified), comparisons, `=` binding, wildcards.
sink(X) :- node(X), not edge(X, _).
big(X)  :- n(X), X >= 5, X != 10.
boxed(W) :- sat(A), W = box(A).

% Aggregates in the head, grouped by the other head arguments.
n_out(X, count<Y>) :- edge(X, Y).
```

Variables start with an uppercase letter or `_`; identifiers and symbols start lowercase. Comments start with `%` or `//`. Declaration annotations (`@requirement`) must be on the same line as the declaration's `)`; rule annotations can go on the line above the rule.

**Terms.** Arguments can be compound terms such as `handler(get, "/posts")`, so atoms can be passed to other predicates (`sat(handler(get, P))`). A zero-arity atom `token_auth` is the symbol `token_auth` when used as a term.

**Checks** (run on every change, before anything is written):

- predicates are declared, arities match, and constants fit their declared types;
- *safety*: every variable in the head, in a negated atom, or in a comparison is bound by a positive atom or by `=` from a bound side (wildcards in negated atoms are fine);
- *stratification*: no negation or aggregation inside a recursive cycle;
- *finiteness*: in a recursive rule, variables inside a constructed compound term (in the head, or in `=`) must be bound by a non-recursive atom, so recursion can't build ever-deeper terms.

**Semantics.** Strata are strongly connected components of the dependency graph, evaluated in dependency order; recursive ones use semi-naive iteration. Aggregates range over the distinct satisfying assignments of the body's variables (so `sum<P>` over two items with the same price counts both). Groups with no matches produce no row (no `count` of 0). Each derived fact keeps one proof, from the earliest round it appeared in.

## The requirements layer

Mark requirement predicates with `@requirement`, and use `@requirement` rules to decompose them:

```prolog
.decl resource(name: symbol) @requirement
.decl handler(method: symbol, path: string) @requirement
.decl collection_path(res: symbol, path: string)
.decl item_path(res: symbol, path: string)

@requirement
resource_crud: resource(R) :-
    collection_path(R, P1), item_path(R, P2),              % guards
    handler(get, P1), handler(post, P1),                   % children (AND)
    handler(get, P2), handler(put, P2), handler(delete, P2).

@requirement auth_via_session: authenticated_requests :- session_auth.   % OR:
@requirement auth_via_token:   authenticated_requests :- token_auth.     % two rules, same head
```

In a requirement rule, body atoms of requirement predicates are *children*; everything else (plain atoms, negations, comparisons) is a *guard*. Guards decide which instances exist and must bind every variable in the head and children. The rule is expanded, with `D = name(vars of the head and children)`:

```prolog
resource_crud__candidate: candidate(D, resource(R)) :- Guards.
resource_crud__child1:    child(D, handler(get, P1)) :- Guards.     % one per child
resource_crud__sat:       sat(resource(R)) :- Guards, sat(handler(get, P1)), ...
```

`multi reqs dl expand` prints the full compiled program. The prelude adds:

```prolog
sat(A) :- evidence(A, _).
violation(exclusive(A, B))              :- exclusive(A, B), sat(A), sat(B).
violation(excluded_but_satisfied(A))    :- excluded(A), sat(A).
violation(required_and_excluded(A))     :- required(A), excluded(A).
child_total(D, count<C>) :- child(D, C).
child_sat(D, count<C>)   :- child(D, C), sat(C).
```

Rules of use: requirement predicates can't be asserted as facts (use `evidence`, `require`, `exclude`), can't appear directly in plain rules (match `sat(...)` instead), and can't be negated in requirement rules (use `exclusive` or `excluded`). `required`, `excluded`, `evidence`, and `exclusive` facts are type-checked against the requirement declarations. The prelude's predicate names are reserved.

**Status.** An alternative is *satisfied* if it has no children or all are satisfied, *unsupported* if none are, *partial* otherwise. An atom is *satisfied* if `sat(atom)` holds, *partial* if some alternative has at least one satisfied child, otherwise *unsupported*. Stance is three-valued: `required`, `excluded`, or unknown (no fact).

## CLI

Every command is a subcommand of `multi reqs`. Global flags: `--db PATH` (env `REQS_DB`, default `reqs.db`), `--format text|json`.

| Command | |
|---|---|
| `init` | Create the database and run migrations. |
| `load FILE` | Import a `.dl` file atomically (declarations and rules are upserted by name). |
| `export [--rules] [--facts]` | Print the stored program as `.dl` source; round-trips through `load`. |
| `require ATOM`, `exclude ATOM`, `unset ATOM` | Set or clear an atom's stance. |
| `evidence add ATOM --source S`, `evidence list [ATOM]`, `evidence rm ATOM [--source S]` | Manage evidence. |
| `exclusive A B [--rm]` | Declare (or remove) mutual exclusion; reported as a violation, not enforced. |
| `compute` | Expand, check, evaluate, and store the model. |
| `status [--only satisfied\|partial\|unsupported] [--required]` | Table of every atom the model mentions. |
| `trace ATOM` | Upward: everything that depends on the atom, with what's missing at each step. |
| `explain ATOM [--depth N]` | Downward: alternatives and children. |
| `violations` | Violations with the facts that caused them. |
| `dl decl add SRC [--replace]`, `dl decl list`, `dl decl rm NAME` | Declarations. |
| `dl rule add NAME SRC`, `list [--head P]`, `show`, `edit NAME SRC`, `rename OLD NEW`, `rm` | Rules. |
| `dl fact add ATOM`, `dl fact list [--predicate P]`, `dl fact rm ATOM` | Facts. |
| `dl check` | Validate the program after expansion. |
| `dl expand` | Print the compiled program. |
| `dl query PATTERN` | Match a pattern with variables against the model, e.g. `'child_sat(D, N)'`. |
| `dl why ATOM` | Proof tree for a fact in the model. |

Every mutation loads the program, applies the change in memory, compiles and checks the result, and only then writes it, in one transaction. Rejected changes leave the database untouched. Report commands warn on stderr when the program changed after the last `compute`.

## Storage

`datalog-sqlite` implements:

```rust
pub trait Store {
    type Error;
    fn load_program(&self) -> impl Future<Output = Result<Program, Self::Error>> + Send;
    fn apply(&self, changes: &[Change]) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn save_model(&self, model: &Model) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn load_model(&self) -> impl Future<Output = Result<Option<ModelSnapshot>, Self::Error>> + Send;
}
```

Rules and declarations are stored as canonical source text. Ground terms are hash-consed in `term` (unique on canonical text, arguments in `term_arg`), and facts, model facts, and proof premises refer to them by id, so they can be joined in SQL. `meta` holds a program generation (bumped by every effective change) and the generation the stored model was computed from. Unreferenced terms are garbage-collected when a model is saved. The schema is in `crates/reqs/datalog-sqlite/src/migration.rs`.

## Limitations

- Joins are nested loops over ordered sets; there are no indexes. Fine for thousands of facts (about 11k derived facts in a second or so in a debug build), not for millions.
- `compute` re-evaluates everything; there is no incremental maintenance.
- The finiteness check is conservative: destructuring a recursively bound term through `=` with a compound on one side is rejected even though it's safe.
- One proof per derived fact. Other derivations exist in the model (e.g. via `candidate`/`child`) but aren't recorded as proofs.
- Aggregates produce nothing for empty groups, and only one aggregate is allowed per head.
- Predicates are identified by name alone (no overloading by arity).

## Tests

`cargo make test` runs unit tests for the parser, checks, evaluator, requirement compilation and reports, a SQLite round-trip, and an end-to-end CLI test (`tests/reqs.rs`, driving `multi reqs`) against `crates/reqs/examples/api.dl`.
