# gems-db.md — ActiveRecord + Sequel parity, at compile time

DB parity matrix: the ActiveRecord and Sequel API surface mapped onto a
**compile-time Ruxen ORM** built over the [`ledger`](../../../ledger) SQLite
layer. Ruby's ORMs are the canonical case of runtime metaprogramming
(`define_method` for every attribute, `method_missing` for dynamic finders,
class macros for `has_many`/`validates`/`scope`). Ruxen is a pure compiler with
no interpreter and no runtime class mutation, so every one of those must become
code the compiler **generates**, so the emitted binary has real, typed methods
and never mutates itself. See [README.md](README.md) for the three-tier
framework; this file is tier 2 (compile-time parity) end to end.

Status legend: `NOT-STARTED` (nothing built), `PRIMITIVE-PRESENT` (the
underlying machinery exists, the ORM feature on top does not). Nothing in this
matrix is `DONE` yet: the ORM package does not exist. `Needs` vocabulary:

- **`.rx only`** — pure Ruxen over `ledger`, zero compiler change. All the
  machinery (mixins, generics, blocks, `Result`, `match`) is present today.
- **`.rx + Model synth`** — pure `.rx`, but leans on the one new compiler
  feature (the `Model` field-walk synthesis, defined below).
- **`Model synth`** — must be produced by that synthesis pass itself.
- **`new FFI`** — needs a new C runtime driver (Postgres/MySQL).
- **`new stdlib`** — needs a primitive from `std` (e.g. a mutex, a fuller
  `Time`).

---

## The design truth for the DB layer (read first)

Ruby's `ActiveRecord::Base` gets its power from three runtime tricks. Each has a
single compile-time answer:

| Ruby runtime trick | What it does | Ruxen compile-time answer |
|---|---|---|
| `define_method(col)` per column, driven by reading the DB schema at boot | typed attribute getters/setters | **Field-walk synthesis**: the model's declared fields ARE the schema; the compiler walks them and emits real accessors + a column list |
| `method_missing` → `find_by_name_and_age(...)` | dynamic finders | A **typed chainable `Relation`** replaces every dynamic finder with a compile-time-typed builder that binds params |
| class macros (`has_many`, `validates`, `scope`) evaluated at class-load | associations, validation set, named queries | **Declarations the synthesis reads** and turns into generated methods, or plain `.rx` methods where a hand-written body is as short as the macro |

The crux, and the only irreducible new compiler work, is the first row.
Ruxen has **no compile-time field reflection today**: nothing except the
internal derive walker (`compiler/ruxen_core/src/mir/lower/derive.rs`, which
already walks fields to synthesize `Clone`/`Debug`/`eq`/`hash`) can enumerate a
class's fields. A model needs that walk exposed as a new synthesized surface.
Once a model exposes `table_name`, `columns`, `to_row`, and `from_row` as real
typed methods, **everything else in this document is plain `.rx`** over `ledger`
plus generics: the query builder, finders, CRUD, associations, validations,
scopes, transactions, and serialization all compile against those four methods
with no further compiler support.

So the whole ORM factors into: **one new derive** (the schema surface), then a
`.rx` library on top.

---

## The one new compiler feature: the `Model` synthesis

Declared like any Ruxen class, with an opt-in `include Model` (the same
in-body `include` used for `Debug`/`Clone` today):

```ruxen
class Todo
  include Model

  id:         Int
  label:      String
  done:       Bool
  created_at: Time
  updated_at: Time
end
```

`include Model` is gated and lowered by extending the existing auto-include
machinery (`compiler/ruxen_core/src/implicit_includes/mod.rs` for eligibility +
diagnostics, `mir/lower/derive.rs` for body synthesis). From the field walk the
compiler generates, as real MIR methods on `Todo`:

| Generated member | Shape | Derived from |
|---|---|---|
| `def self.table_name -> String` | `"todos"` | class name, snake_cased + pluralized (overridable by defining it) |
| `def self.columns -> Array[String]` | `["id","label","done","created_at","updated_at"]` | field names in declaration order |
| `def self.column_types -> Array[String]` | SQL types `["INTEGER","TEXT","INTEGER",...]` | per-field-type map (`Int→INTEGER`, `String→TEXT`, `Bool→INTEGER`, `Float→REAL`, `Time→TEXT`, `Bytes→BLOB`) |
| `def to_row -> Array[String]` | each field stringified for `exec_params` binding | per-field encoder (`Int.to_s`, `Bool → "1"/"0"`, `Time → iso8601`) |
| `def self.from_row(row: &Json) -> Self` | decode one JSON object into `Self` | per-field decoder (`JSON.as_int`, `JSON.as_string`, `"1" → true`) |
| `def self.build(...) -> Self` | keyword-ish constructor over non-`id` fields | field list minus the primary key |
| `def to_json -> Json` | `JSON.object` node, field by field | same walk (folds serialization in, see §12) |

Fields are public by default in Ruxen, so `todo.label` already reads; the
synthesis adds the *collection* surface (`columns`/`to_row`/`from_row`) that
generic code needs, not per-field getters (those already exist as field
access). Per-field encoders/decoders reuse exactly the pattern
`derive.rs` already uses to emit per-field `eq`/`hash`, so this is an extension
of a proven walker, not a new subsystem. Primary key defaults to `id: Int`;
`include Model, primary_key: :uuid` (a header option, mirroring the `include
Debug, Clone` list form) overrides it.

This is the sole non-negotiable compiler ask. Track it as a language task
(`ruxen/docs/TASKS.md`, a new `Q##` + `gui-stack-v1-issues.md` entry) since it
extends the derive registry.

---

## 1. Models & schema declaration

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `class User < ApplicationRecord` with dynamic attribute accessors read from the DB | `class User` with `include Model`; declared fields are the schema; accessors are plain public fields | Model synth | NOT-STARTED |
| `attr_accessor`-style getters/setters | Fields public by default (direct `user.name`); no synthesis needed for read/write | `.rx only` (PRIMITIVE-PRESENT) | NOT-STARTED |
| `self.table_name = "people"` | Default `table_name` generated from class name; override by defining `def self.table_name` | Model synth | NOT-STARTED |
| `User.column_names` | Generated `def self.columns -> Array[String]` | Model synth | NOT-STARTED |
| `User.primary_key` | `id: Int` default; header option `primary_key: :uuid` | Model synth | NOT-STARTED |
| `User.new(attrs)` / `User.build` | `.new` (Ruxen `def init`) + generated `self.build` over non-PK fields | Model synth | NOT-STARTED |
| Sequel `Sequel::Model(:users)` (table binding, no field decl) | Not mirrored: Ruxen needs static field types, so fields are always declared. Schema stays the single source of truth | Model synth | NOT-STARTED |
| STI (`type` column) | A `type: String` field + a generated dispatch in `from_row`; subclass filtering via `where("type = ?")`. v2 | Model synth | NOT-STARTED |

Design note: in Rails the DB is the source of truth and the model infers
columns at boot. Ruxen cannot introspect a DB at compile time, so the
**model declaration is the single source of truth**, and migrations are
kept in sync from it (§8 shows generating the `CREATE TABLE` DDL from the same
field walk, so there is one authoritative list).

---

## 2. CRUD / persistence

All of these are generic default methods on the `Model` mixin, bounded by the
synthesized surface. Because `T` exposes `table_name`/`columns`/`to_row`/
`from_row`, each body is plain `.rx` over `ledger` with bound parameters.

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `Model.create(attrs)` | `self.build(...)` then `save`; returns the instance with `id` set from `last_insert_rowid` | `.rx + Model synth` | NOT-STARTED |
| `record.save` / `save!` | `INSERT` (new) or `UPDATE` (persisted) built from `columns`/`to_row`, run via `Db.exec_params`; `save!` returns `Result`, `save` a `Bool` | `.rx + Model synth` | NOT-STARTED |
| `record.update(attrs)` | assign fields, then `save` on the changed columns | `.rx + Model synth` | NOT-STARTED |
| `record.destroy` / `destroy!` | `DELETE FROM t WHERE id = ?` bound | `.rx + Model synth` | NOT-STARTED |
| `record.reload` | re-`SELECT` by PK, overwrite fields from `from_row` | `.rx + Model synth` | NOT-STARTED |
| `Model.find(id)` | `SELECT * WHERE id = ?`, decode one row; `Result`/`Option` return, no exception raised for the missing case | `.rx + Model synth` | NOT-STARTED |
| `Model.find_by(cond)` / `find_by!` | thin wrapper over `where(...).first`; `!` variant returns `Err` when absent | `.rx + Model synth` | NOT-STARTED |
| `Model.all` | `SELECT *`, map `from_row` over the parsed JSON array into `Array[Self]` | `.rx + Model synth` | NOT-STARTED |
| `Model.first` / `last` | `ORDER BY id ASC/DESC LIMIT 1` | `.rx + Model synth` | NOT-STARTED |
| `Model.exists?(cond)` | `SELECT 1 ... LIMIT 1`, test row count | `.rx + Model synth` | NOT-STARTED |
| `Model.count` | `SELECT COUNT(*)`, read scalar | `.rx + Model synth` | NOT-STARTED |
| `Model.find_each` / `in_batches` | keyset pagination loop yielding a block; avoids loading all rows | `.rx + Model synth` | NOT-STARTED |
| `insert_all` / `upsert_all` (bulk) | one multi-VALUES `INSERT` from an `Array[Self]`; `ON CONFLICT` for upsert | `.rx + Model synth` | NOT-STARTED |

Return-shape decision, stated once: Ruby's `find` raises
`RecordNotFound`; `find_by` returns `nil`. Ruxen mirrors the *distinction* but
in its own idiom: the plain forms return `Option[Self]` / `Result[Self,
DbError]` and the `!` forms return `Result` with an `Err(NotFound)`, so callers
use `match`/`?` instead of rescue. This is the compiled-language adaptation the
README sanctions (Ruby semantics, Ruxen mechanism).

Sketch of the generic `all` (illustrates the whole pattern):

```ruxen
mixin Model
  # Provided by the synthesis on each including class:
  def self.table_name -> String
  def self.columns -> Array[String]
  def self.from_row(row: &Json) -> Self

  def self.all(db: Int) -> Result[Array[Self], DbError]
    let sql  = "SELECT * FROM " + Self.table_name + " ORDER BY id"
    let rows = Db.query_json(db, &sql)
    Relation.decode_all(&rows)   # parse JSON array, from_row each element
  end
end
```

---

## 3. Query builder — the typed `Relation`

A lazy, chainable `Relation[T]` that accumulates clauses and only touches the DB
when materialized (`to_a`/`first`/`count`/`each`). It builds a **bound-param**
SQL string (`?` placeholders + a parallel `Array[String]`), so it is
injection-safe by construction, the same guarantee `ledger`'s `exec_params`
gives. This single typed builder replaces the entire `method_missing` dynamic
finder family.

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `.where("age > ?", 18)` (string + placeholder) | `.where(&"age > ?", &["18"])`: fragment appended, params bound. The primary, always-safe form | `.rx + Model synth` | NOT-STARTED |
| `.where(name: "x", active: true)` (hash) | `.where_eq(Hash[String,String])`: each pair → `col = ?` AND-joined. Homogeneous `String→String` hash covers equality; richer conditions use the fragment form or generated column predicates | `.rx + Model synth` | NOT-STARTED |
| `.where(id: [1,2,3])` (IN) | `.where_in(&"id", &["1","2","3"])` → `id IN (?,?,?)` | `.rx + Model synth` | NOT-STARTED |
| `.where.not(...)` | `.where_not(&"status = ?", &["x"])` | `.rx + Model synth` | NOT-STARTED |
| `.or(other_relation)` | `.or(...)`: wrap two clause groups in `( ... OR ... )` | `.rx + Model synth` | NOT-STARTED |
| `.order(:name)` / `.order(name: :desc)` | `.order(&"name")` / `.order_desc(&"name")` | `.rx + Model synth` | NOT-STARTED |
| `.limit(n)` / `.offset(n)` | `.limit(n)` / `.offset(n)` | `.rx + Model synth` | NOT-STARTED |
| `.group(:x).having("...")` | `.group(&"x").having(&"count(*) > ?", &["5"])` | `.rx + Model synth` | NOT-STARTED |
| `.joins(:posts)` | `.joins(&"INNER JOIN posts ON posts.user_id = users.id")`; association-aware `.joins_assoc` once associations land | `.rx + Model synth` | NOT-STARTED |
| `.includes(:posts)` (eager load, N+1 kill) | two-query batched preload: run the base query, collect ids, `SELECT * FROM posts WHERE user_id IN (...)`, stitch in `.rx`. See §5 | `.rx + Model synth` | NOT-STARTED |
| `.pluck(:name)` | `SELECT name`, decode a single-column `Array[String/Int]` | `.rx + Model synth` | NOT-STARTED |
| `.select(:a, :b)` | override the projection list | `.rx + Model synth` | NOT-STARTED |
| `.distinct` | `SELECT DISTINCT` | `.rx + Model synth` | NOT-STARTED |
| chained + lazy | `Relation` is a value; each method returns a new `Relation` (Clone-derived); nothing runs until a terminal | `.rx only` (PRIMITIVE-PRESENT) | NOT-STARTED |
| `.to_sql` | return the built SQL string + a debug of the bound params | `.rx only` | NOT-STARTED |
| Sequel `DB[:users].where{age > 18}` (virtual-row block) | Ruxen has no runtime `method_missing` on a virtual row; a block form needs generated column tokens (below). v1 uses the fragment form | Model synth (typed columns) | NOT-STARTED |
| Typed column predicates `User.col(:age).gt(18)` | The synthesis additionally emits a column-token per field (`User.age_col`) carrying name + type, so `.where(User.age_col.gt(18))` is fully typed and needs no raw string. The AR-parity endgame for the builder | Model synth (typed columns) | NOT-STARTED |

Usage the way a Rails dev expects, all lazy:

```ruxen
let overdue = Todo.where(&"done = ?", &["0"])
                  .where(&"created_at < ?", &[cutoff.iso8601])
                  .order_desc(&"created_at")
                  .limit(20)
overdue.to_a(db)   # <- only here does SQL run
```

Two builder tiers are offered on purpose: the **bound-fragment** form works the
day the `Model` synth lands (pure `.rx`, stringly column names but always
injection-safe); the **typed-column** form is a later synthesis extension that
emits a descriptor per column so column names are checked by the compiler too.

---

## 4. Associations

Declared on the model. Each declaration is read by the `Model` synthesis and
turned into a generated accessor method that returns a typed `Relation` or a
single record. No `define_method`, no runtime.

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `belongs_to :user` | declaration → `def user(db) -> Result[Option[User], DbError]` = `User.find(self.user_id)`; generates the `user_id: Int` foreign-key expectation | Model synth | NOT-STARTED |
| `has_many :posts` | → `def posts -> Relation[Post]` = `Post.where(&"user_id = ?", &[self.id.to_s])` | Model synth | NOT-STARTED |
| `has_one :profile` | → `def profile(db) -> Option[Profile]` = `Profile.where(...).first` | Model synth | NOT-STARTED |
| `has_many :through` | → `def tags -> Relation[Tag]` emitting the join through the join table | Model synth | NOT-STARTED |
| polymorphic `belongs_to :subject, polymorphic: true` | `subject_type: String` + `subject_id: Int` fields; generated accessor `match`es on `subject_type` to the right model's `find`. Closed set of target models declared with the association | Model synth | NOT-STARTED |
| `dependent: :destroy` | generated `destroy` also deletes children in the same transaction | Model synth | NOT-STARTED |
| eager load `includes(:posts)` (the N+1 fix) | batched two-query preload in `.rx`: fetch parents, gather PKs, one `WHERE fk IN (...)`, group children by fk, attach into a `preloaded` slot on each parent | `.rx + Model synth` | NOT-STARTED |
| `counter_cache` | generated `save`/`destroy` bump a `*_count` column in the same tx | Model synth | NOT-STARTED |

Hand-written equivalent (what the generated `has_many` expands to, so the
codegen target is obvious and a user can always drop to it):

```ruxen
def posts -> Relation[Post]
  Post.where(&"user_id = ?", &[self.id.to_s])
end
```

Because the expansion is this short, associations are a thin synthesis: the
compiler reads the `has_many :posts` declaration and emits exactly that method.
Eager loading is the only associative feature with real logic, and it stays in
`.rx` (a two-query batch), not in the compiler.

---

## 5. Validations

Declared, then compiled into a generated `valid?` / `errors`. Ruby runs the
validation set through reflection; Ruxen generates a straight-line `valid?` body
from the declarations.

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `validates :name, presence: true` | generated check `self.name.blank?` → push error | Model synth | NOT-STARTED |
| `validates :email, uniqueness: true` | generated `SELECT 1 WHERE email = ? AND id <> ?` count check (needs `db`); paired with a DB unique index for the race (see below) | `.rx + Model synth` | NOT-STARTED |
| `validates :name, length: {minimum: 2, maximum: 40}` | generated bounds check on `name.length` | Model synth | NOT-STARTED |
| `validates :email, format: {with: /.../}` | generated `Regexp` match | Model synth + `std/regexp` | NOT-STARTED |
| `validates :age, numericality: {greater_than: 0}` | generated numeric comparison | Model synth | NOT-STARTED |
| custom `validate :some_method` | generated `valid?` calls the user-defined method if the class defines it (the synthesis has the method list) | Model synth | NOT-STARTED |
| `errors.full_messages` | generated `errors: Array[String]` accumulator; `valid? -> Bool` clears then fills it | Model synth | NOT-STARTED |
| `save` runs validations | generated `save` calls `valid?` first, returns `Err(Invalid(errors))` on failure | `.rx + Model synth` | NOT-STARTED |

Concurrency truth, stated plainly: a `uniqueness` validation is a check, not a
guarantee. Two requests in the same second both pass the `SELECT` and both
`INSERT`. The generated migration for a `uniqueness` validation therefore
**also emits a `UNIQUE` index** (§8), and `save` treats the unique-constraint
violation from `ledger` as the authoritative `errors` entry. The model is the
last line of defense; the index is the actual lock.

---

## 6. Callbacks

Generated hook points inside the generated `save`/`create`/`update`/`destroy`.
The synthesis knows the class's method list, so it emits a call only when the
hook is defined (the compile-time analogue of Rails registering a callback).

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `before_save` / `after_save` | generated `save` calls `self.before_save` (if defined), does the write, then `self.after_save` | Model synth | NOT-STARTED |
| `before_create` / `after_create` | same, on the INSERT path only | Model synth | NOT-STARTED |
| `before_update` / `after_update` | same, on the UPDATE path only | Model synth | NOT-STARTED |
| `before_destroy` / `after_destroy` | wrap the DELETE | Model synth | NOT-STARTED |
| `before_validation` | called ahead of `valid?` | Model synth | NOT-STARTED |
| halting a callback (`throw :abort`) | hook returns `Result`; an `Err` aborts the save and rolls back the tx | Model synth | NOT-STARTED |
| `after_commit` / `after_rollback` | fired by the `transaction` helper (§9) after COMMIT/ROLLBACK | `.rx + Model synth` | NOT-STARTED |

No conditional-callback DSL (`if:`/`unless:`) in v1: the user writes the guard
inside the hook method. Keeps the synthesis reading a fixed set of well-known
method names rather than a mini-language.

---

## 7. Migrations & schema DSL

A pure `.rx` DDL builder. A migration is a class with `up`/`down` (or a
reversible `change`), and each helper accumulates a `CREATE TABLE`/`ALTER`
string run through `Db.exec`. Blocks (`do …end`, immediately-invoked) give the
Rails feel with no compiler support.

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `create_table :users do |t| ... end` | `create_table(db, &"users") do |t: &var Table| ... end` builds + execs DDL | `.rx only` | NOT-STARTED |
| `t.string/integer/boolean/datetime/...` | `t.string(&"name")`, `t.integer(&"age")`, etc. append a column def | `.rx only` | NOT-STARTED |
| `t.timestamps` | appends `created_at`/`updated_at TEXT` | `.rx only` | NOT-STARTED |
| `t.references :user` (FK column + index) | `t.references(&"user")` → `user_id INTEGER` + index | `.rx only` | NOT-STARTED |
| `add_column` / `remove_column` / `rename_column` | `alter_table` helpers emitting `ALTER TABLE` | `.rx only` | NOT-STARTED |
| `add_index` / `add_index unique: true` | `add_index(db, &"users", &["email"], unique: true)` | `.rx only` | NOT-STARTED |
| reversible `change` / `up`+`down` | mirror global rule 12: `change` for the reversible subset, explicit `down` otherwise. Migration runner runs `down` on rollback | `.rx only` | NOT-STARTED |
| migration versioning / `schema_migrations` | a `schema_migrations` table tracks applied version strings; runner skips applied ones | `.rx only` | NOT-STARTED |
| `db:migrate` runner | a `.rx` function (or a `ruxen`-invoked script) that loads pending migrations in order | `.rx only` | NOT-STARTED |
| Sequel `DB.create_table` / migrations dir | same builder; Sequel's API maps onto the same helpers | `.rx only` | NOT-STARTED |
| **generate schema from the model** | the `Model` field-walk emits a `create_table` DDL, so model fields are the one source of truth for both accessors and DDL | Model synth | NOT-STARTED |

Because the migration builder is entirely string assembly over `ledger`, it is
the part with the **least** compiler risk and a good first shippable piece.

```ruxen
class CreateTodos < Migration
  def change(db: Int) -> Result[nil, DbError]
    create_table(db, &"todos") do |t: &var Table|
      t.primary_key(&"id")
      t.string(&"label")
      t.boolean(&"done")
      t.timestamps
    end
  end
end
```

---

## 8. Transactions

A block helper that brackets `BEGIN`/`COMMIT` and rolls back on any `Err`.
Immediately-invoked `do …end` fits exactly.

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `ActiveRecord::Base.transaction do ... end` | `transaction(db) do ... end`: `BEGIN`; run block; `COMMIT`, or `ROLLBACK` if the block returns `Err`/raises | `.rx only` | NOT-STARTED |
| nested transactions / savepoints | `SAVEPOINT`/`RELEASE`/`ROLLBACK TO` when already inside a tx | `.rx only` | NOT-STARTED |
| `requires_new: true` | force a savepoint | `.rx only` | NOT-STARTED |
| isolation levels | pass through as `PRAGMA`/`BEGIN` variants per adapter | `.rx only` | NOT-STARTED |
| rollback propagation to in-memory state | block returns `Result`; caller decides. No autoload of reverted attributes in v1 | `.rx only` | NOT-STARTED |

```ruxen
transaction(db) do
  from.withdraw(amount)?
  to.deposit(amount)?
  Ok(nil)
end   # COMMIT on Ok, ROLLBACK on the first ? that short-circuits an Err
```

---

## 9. Connection pool

`ledger` identifies a connection by an opaque `Int` handle. A pool is an array
of handles with checkout/checkin, which matters because `rondo` is
thread-per-core: each request needs its own handle (SQLite connections are not
safe to share across threads concurrently).

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| connection pool with checkout/checkin | `Pool` holding `Array[Int]` handles + a guard; `pool.with(db) do ... end` checks one out and returns it | `.rx only` + `new stdlib` (mutex) | NOT-STARTED |
| `pool_size` / timeout config | fields on `Pool`; block until a handle frees or time out | `.rx only` + `new stdlib` | NOT-STARTED |
| per-thread / per-request connection | `rondo` checks out one handle per request, checks it back in on response | `.rx only` | NOT-STARTED |
| `ActiveRecord::Base.connection` | ambient current handle: passed explicitly in v1 (a `db: Int` arg threaded through), not a global. A `Current`-style thread-local is a later ergonomic | `.rx only` | NOT-STARTED |
| reconnect / dead-connection eviction | pool health-checks a handle on checkout | `.rx only` | NOT-STARTED |

The one honest dependency here is a **mutex/condition primitive** from `std`
for the pool guard. Check what `SharedSync` (used in `quiver`) already provides
before adding one. Threading `db` explicitly through calls in v1 sidesteps the
need for a thread-local ambient connection.

---

## 10. Adapters beyond SQLite

`ledger` is SQLite-only FFI (vendored amalgamation). Postgres/MySQL each need a
new C runtime driver (libpq, libmysqlclient) behind the **same interface**, so
the ORM stays adapter-agnostic. Model this as an `Adapter` mixin that the
`Relation` SQL generator talks to, so dialect differences never leak into the
model or query layer.

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| SQLite adapter | `ledger` as-is | PRIMITIVE-PRESENT | NOT-STARTED |
| Postgres adapter | new FFI over libpq; `Adapter` mixin: `$1` placeholders, `RETURNING id`, `"ident"` quoting, real types/arrays | new FFI | NOT-STARTED |
| MySQL adapter | new FFI over libmysqlclient; backtick quoting, `?` placeholders | new FFI | NOT-STARTED |
| dialect abstraction (placeholder style, RETURNING, quoting, upsert) | `Adapter` mixin methods (`placeholder(n)`, `quote_ident`, `insert_returning?`); `Relation` calls them so one builder serves all backends | `.rx only` (mixin) | NOT-STARTED |
| `last_insert_id` differences | `Adapter` method: SQLite `last_insert_rowid()` vs Postgres `RETURNING` | `.rx only` + per-FFI | NOT-STARTED |
| connection URL parsing (`postgres://...`) | a `.rx` URL parser choosing the adapter | `.rx only` | NOT-STARTED |

FFI is the cost center for multi-adapter support. The `.rx` layers above the
`Adapter` mixin do not change per backend, which is the payoff of routing all
dialect knowledge through one mixin (`Q17`-style per-implementor
monomorphization already supports N backends from one generic query layer).

---

## 11. Serialization

No JSON derive exists today (`std/json` is a manual facade). The `Model`
field-walk already visits every field, so it folds serialization in for free.

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `to_json` | generated `def to_json -> Json` building a `JSON.object` node field by field | Model synth | NOT-STARTED |
| `as_json(only:/except:/methods:)` | generated `to_json_with(fields: Array[String])` honoring an allow/deny list | Model synth | NOT-STARTED |
| `from_row` / hydrate from DB | already generated (§ the synthesis) as `self.from_row(&Json)` | Model synth | NOT-STARTED |
| serialize a whole `Relation`/`Array[Model]` | map `to_json` and wrap in `JSON.array` | `.rx only` | NOT-STARTED |
| `serialize :prefs, coder: JSON` (a JSON column) | a `Json`-typed field encoded as a TEXT column via `to_row`/`from_row` | Model synth | NOT-STARTED |

This directly serves `rondo`: a controller can `render(model.to_json)` with no
hand-built JSON, closing a gap noted in `std/json` (serializing a user class is
manual today).

---

## 12. Scopes, enums, timestamps, dirty tracking

| AR/Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `scope :active, -> { where(active: true) }` | a plain class method returning a `Relation`: `def self.active -> Relation[Self]; where(&"active = ?", &["1"]); end`. A `scope` macro is optional sugar over this | `.rx + Model synth` | NOT-STARTED |
| default scope | a generated base `Relation` all queries start from. Use sparingly (same footguns as Rails) | Model synth | NOT-STARTED |
| `enum status: {draft: 0, live: 1}` | a real Ruxen `enum` field; `to_row`/`from_row` map tag ↔ column int/string. Predicates `status.live?` / setters are `match` arms or generated | Model synth | NOT-STARTED |
| enum bang/predicate (`user.active!`, `active?`) | generated per-value predicate + setter, or hand-written `match`. Ruxen enums make this natural | Model synth | NOT-STARTED |
| `timestamps` (`created_at`/`updated_at` auto-set) | generated `create` sets both, `update` sets `updated_at`; needs a usable `Time` | `.rx + Model synth` + `std/time` | NOT-STARTED |
| `touch` | `UPDATE ... SET updated_at = ?` | `.rx + Model synth` | NOT-STARTED |
| dirty tracking `changed?` / `name_was` | on load, keep a `Clone`-derived snapshot; `changed?` compares live vs snapshot (whole-record cheaply via the existing `Clone`/`PartialEq` derives); per-field `name_changed?` is a later synthesis extension | `.rx + Model synth` (whole-record) / Model synth (per-field) | NOT-STARTED |
| `previous_changes` after save | swap the snapshot on save, expose the prior copy | Model synth | NOT-STARTED |

Whole-record dirty checking is nearly free: `Clone` and `PartialEq` already
derive today, so a snapshot-and-compare needs no new compiler work. Per-attribute
dirty tracking (`name_was`) is the only piece that wants the field walk.

---

## 13. Sequel-specific surface

Sequel is dataset-first (`DB[:users].where(...).all`), with the model layer
optional. The `Relation` above already IS a dataset, so Sequel maps cleanly.

| Sequel feature | Compile-time Ruxen design | Needs | Status |
|---|---|---|---|
| `DB[:users]` dataset without a model | a `Table` dataset value producing `Array[Json]`/`Hash` rows, no typed model required | `.rx only` | NOT-STARTED |
| `dataset.where(...).all` | same `Relation` machinery, untyped row output | `.rx only` | NOT-STARTED |
| `dataset.insert/update/delete` | `Db.exec_params` wrappers on the dataset | `.rx only` | NOT-STARTED |
| virtual-row blocks `where{age > 18}` | needs generated column tokens (see §3 typed columns); fragment form until then | Model synth (typed columns) | NOT-STARTED |
| `Sequel::Model` plugins system | not mirrored: plugins are runtime module injection. The equivalent is `include`-ing extra mixins at compile time | `.rx only` (mixins) | NOT-STARTED |
| prepared statements / bound variables | `ledger` already binds params; a named-prepared cache is a later FFI addition | PRIMITIVE-PRESENT / new FFI | NOT-STARTED |
| `DB.transaction` | same `transaction` helper (§8) | `.rx only` | NOT-STARTED |

---

## Build order (implementation sequence)

The stack factors so that the largest possible surface is pure `.rx`, with a
single compiler feature (the `Model` synthesis) in the middle. Ship in phases,
each with tests + a `tests/release-e2e/` pin per the repo discipline.

**Phase 0 — prove the stack in pure `.rx`, zero compiler change.**
Write the `Relation` (bound-fragment builder → SQL string) and row-decode
helpers over `ledger`, plus a hand-written model example (a class where
`table_name`/`columns`/`to_row`/`from_row` are written by hand). This proves
`find`/`all`/`create`/`where`/`transaction`/migrations all work end to end with
no compiler support, at the cost of per-model boilerplate. Everything here
survives into the real ORM unchanged; only the hand-written glue gets replaced.

**Phase 1 — the one compiler feature: `Model` field-walk synthesis.**
Extend `implicit_includes` (gating/diagnostics) + `mir/lower/derive.rs` (body
synthesis) to emit `table_name`, `columns`, `column_types`, `to_row`,
`from_row`, `build`, and `to_json` from a model's declared fields. Register it
as a new `Q##` language task. After this lands, the Phase-0 hand-written glue is
deleted and the `Relation` binds generically to any `include Model` class.

**Phase 2 — the `.rx` ORM core on the synthesized surface.**
The `Model` mixin's generic CRUD (`save`/`create`/`update`/`destroy`/`reload`/
`find`/`find_by`/`all`/`first`/`last`/`exists?`/`count`) and the full lazy
`Relation` (where/not/or/order/limit/offset/group/having/joins/pluck/select/
distinct/to_sql). All `.rx`; the synthesis makes it compile.

**Phase 3 — migrations + transactions (pure `.rx`, can run parallel to Phase 2).**
The DDL builder, the migration runner + `schema_migrations`, the `transaction`
helper, and generate-DDL-from-model. Lowest compiler risk, high value; a good
early ship.

**Phase 4 — associations + eager loading.**
Extend the synthesis to read `belongs_to`/`has_many`/`has_one`/`through`/
polymorphic declarations and emit the accessor methods; `includes` batched
preload stays in `.rx`.

**Phase 5 — validations + callbacks.**
Extend the synthesis to read `validates`/callback declarations and generate
`valid?`/`errors` + the hook calls inside `save`/`create`/`update`/`destroy`.
Pair `uniqueness` with an emitted `UNIQUE` index.

**Phase 6 — ergonomics: scopes, enums, timestamps, dirty tracking, serialization,
typed-column predicates.** Mostly synthesis extensions over the same walk.

**Phase 7 — multi-adapter.**
The `Adapter` mixin + Postgres (libpq) and MySQL (libmysqlclient) FFI drivers +
the connection pool (needs a `std` mutex). The `.rx` layers above the adapter do
not change.

Critical path: **Phase 1 is the only hard blocker**; Phases 0, 3 ship without
it, and everything from Phase 2 on is `.rx` gated solely on Phase 1.

---

## Package name

Siblings in the stack are short evocative nouns: `ledger`, `quiver`, `rondo`,
`canvas`. `ledger` is the raw SQLite store; the ORM is the record-mapping layer
that sits on it. Suggested name: **`folio`** (a folio is a numbered leaf/record
in a ledger book, so `folio` on `ledger` reads exactly right: models are the
pages of the ledger). Alternates if `folio` clashes: **`scribe`** (writes and
reads records) or **`roster`**. The rest of this doc assumes `folio`.

Dependency direction: `folio → ledger → (ruxen toolchain)`, mirroring
`quiver → canvas`. Apps (`rondo`, and any GUI app needing persistence) depend on
`folio`.

---

## Status summary

- **Everything here is `NOT-STARTED`.** The `folio` package does not exist; no
  model/schema/migration/association code exists anywhere in the workspace; this
  matrix file is the design, not an inventory of built features.
- **Primitives present today** (the foundation to build on): `ledger`'s
  injection-safe SQLite FFI; Ruxen `mixin` + generics with per-implementor
  monomorphization (`Q17`); `alias`; Ruby blocks/`yield`; `Result`/`Option`/
  `match`/`?`; and the derive walker in `mir/lower/derive.rs` that the `Model`
  synthesis extends.
- **The single new compiler feature** is the `Model` field-walk synthesis
  (Phase 1). It is an extension of the existing derive machinery, not a new
  subsystem, and it unblocks the entire `.rx` ORM above it.
- **FFI work** (Postgres/MySQL) and **one `std` primitive** (a pool mutex) are
  the only other non-`.rx` costs, both isolated behind a mixin/adapter boundary.

_Created 2026-07-23 as part of the Ruby-parity audit (see [README.md](README.md))._
