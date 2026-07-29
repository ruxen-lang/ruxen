# B54 design spec: folio, the Ruxen ORM

Design document for the folio line, QUEUE.md batches **B54 through B61**.
Design only: no Rust, no `.rx` implementation.

**Why the shape matters more than the features.** This is the largest remaining
design gap and it has the most downstream code. `gems-db.md` is a good target
surface, but it was written from intent rather than from probes, and **its
central design sketch does not compile**. This spec re-derives the architecture
from what the compiler actually does today.

All probes were run on 2026-07-29 with `./target/release/ruxen compile` (and
`ruxen run` for the ledger stack), each in its own file per B75.

---

## 1. Probe results

### 1.1 `ledger` is real, minimal, and works end to end

`~/.projects/ledger` exists with vendored SQLite (`runtime/sqlite3.c`, so no
system dependency). Its entire public surface is six class methods on `Db`:

| Method | Shape |
|---|---|
| `Db.open(path)` | `Result[Int, String]`, an opaque integer handle |
| `Db.exec(handle, sql)` | `Result[nil, String]` |
| `Db.exec_params(handle, sql, params: &Array[String])` | `Result[nil, String]` |
| `Db.query_json(handle, sql)` | `String` (a JSON array) |
| `Db.query_json_params(handle, sql, params: &Array[String])` | `String` |
| `Db.close(handle)` | `nil` |

**Verified running** (`ruxen run` in `examples/smoke`, 7.09s build):

```
rows json: [{"id":1,"label":"buy milk","done":0},{"id":2,"label":"walk o'dog","done":1},…]
done rows: [{"label":"walk o'dog"}]
parsed 3 rows from the db
```

Three facts fall out of that output and the C source:

1. **Reads are typed.** `build_rows_json` emits INTEGER and FLOAT columns as bare
   JSON numbers, NULL as `null`, everything else quoted. So `from_row` receives
   properly typed JSON.
2. **Writes are text-only.** Every parameter is an `&Array[String]` bound with
   `sqlite3_bind_text`. An `Int` column is written as `"42"`. SQLite's type
   affinity converts on insert, but a comparison against a non-affinity
   expression can behave differently, so this is a real limitation (§6).
3. **The quote in `walk o'dog` survived**, which proves binding rather than
   splicing.

**Two gaps, both confirmed by grep:** there is **no `last_insert_rowid`** exposed
(`grep` returns 0 hits in both `src/lib.rx` and `runtime/sqlite.c`), so
`create` cannot return the id it just generated, and there is **no
BEGIN/COMMIT/ROLLBACK helper** (though `Db.exec(db, "BEGIN")` works).

### 1.2 `where` is a reserved keyword (R1)

```
def where(frag: &String) -> Relation[T]
  error: expected expression, found RParen
  error: expected identifier, found Where
```

`where` is taken by generic bounds (`where T: Add`). **The single most important
method name in the entire ORM surface cannot be spelled.** Renaming to `filter`
compiles immediately (R2), so nothing else about the shape is wrong.

### 1.3 A phantom-typed lazy `Relation[T]` works today (R2)

```
Relation[User].new("SELECT * FROM users").filter(&"age > ?").limit(10)
# prints: SELECT * FROM users WHERE age > ? LIMIT 10
```

A generic class carrying a type parameter it never stores, with chaining methods
returning `Relation[T]`, compiles and runs. **The typed lazy builder is
expressible in pure `.rx` right now.**

### 1.4 Static dispatch through an abstract type is broken (R3, R4). This is the finding that reshapes the design

| Probe | Shape | Result |
|---|---|---|
| **R3** | `class Relation[T: FromRow]` whose body calls `T.from_row(row)` | `codegen: no runtime symbol for '?T::from_row' (mangled '?T1817_from_row')` |
| **R4** | `mixin Model` with `def self.all` calling `Self.from_row`, **the exact sketch in `gems-db.md` §2** | `Failed to define function 'User_all': Verifier(... "mismatched argument count for v3 = call fn1(v2, v1): got 2, expected 1")` |
| **R5** | mixin default calling a required **instance** method on `Self` | **works**: `row=u3` |
| **R6** | generic function receiving the decoder as a **closure value** | **works**: `n=2 first=9` |

The distinction is exact and it is the whole architecture:

> **Instance methods dispatch through an abstract `Self` or `T`. Static methods
> do not.** A static on an abstract type either fails to find a symbol (R3) or
> emits a call with a receiver the callee does not take (R4).

R4 is also, almost certainly, the same defect a sibling agent reported as "a
Cranelift verifier arity mismatch", so it is not an exotic corner.

**Consequence: `gems-db.md`'s Phase 2 does not compile.** Its design has every
model inherit generic CRUD from `mixin Model`, whose default bodies call
`Self.table_name`, `Self.columns`, and `Self.from_row`. All three are statics on
an abstract `Self`. This is independent of B33 and B55: even after the field walk
synthesizes those methods perfectly, generic code cannot call them.

**R6 is the way out.** A decoder passed as a *value* works today, so folio must
carry model metadata as values rather than dispatching statics on abstract types.

---

## 2. Active-record or repository: repository, and the compiler decides it

### Decision

**folio is a repository/dataset library with an optional, thin active-record
facade.** The core is `Repo[T]` plus `Relation[T]`, parameterised by a **model
descriptor value**. The facade is one-line forwarders on each model, written by
the user or generated later.

### The argument

Ruby developers expect ActiveRecord, and `gems-db.md` assumes it, so this needs a
real reason rather than a preference.

**Active-record is not expressible generically.** `User.all`, `User.find(id)`,
and `User.where(...)` are **statics on the model**. Any *generic* implementation
of them (one body shared by every model) requires calling `Self.from_row` and
`Self.table_name` from a mixin default, which is exactly R4's verifier crash.
There are only three ways out:

1. **Synthesize the CRUD per model in the compiler.** Each model gets its own
   concrete `def self.all`, so no abstract dispatch happens. This works today,
   and it is the anti-pattern [`B33-codegen-substrate.md`](B33-codegen-substrate.md)
   §2 explicitly rejected: fifteen ORM methods become fifteen Rust MIR builders
   carrying SQL semantics into the compiler, which the acceptance grep in
   `generic-compiler.spec.md` forbids.
2. **Wait for static-dispatch-on-abstract-types** (filed as **B77**). Honest, but
   it blocks the entire ORM line behind an unscheduled compiler feature.
3. **Carry the metadata as a value** (R6). Works today, needs no compiler change,
   and keeps the generic CRUD in `.rx` where the north star wants it.

Option 3 is a repository. The model descriptor is an ordinary value:

```
# What B33's field walk provides, wrapped as a value rather than dispatched as statics
struct ModelDesc[T]
  table:   String
  columns: Array[String]
  decode:  any Fn[Fn(&Json) -> T]      # closure, per R6
  encode:  any Fn[Fn(&T) -> Array[String]]
end
```

Everything generic then takes `ModelDesc[T]` as an argument, and never touches a
static on `T`.

**Two further reasons the repository is the better fit anyway**, independent of
the compiler:

- **Ownership.** Active-record wants each record to reach a connection
  (`record.save`). That means either a global ambient connection (which fights
  thread-per-core, §7) or every record holding a handle (which makes every model
  value connection-bound and unusable across a boundary). A repository keeps the
  connection at the call site, where ownership is obvious.
- **`gems-db.md` already conceded the point.** Its own §13 notes "the `Relation`
  above already IS a dataset, so Sequel maps cleanly". The dataset was always the
  real core; active-record was the wrapper.

### The facade, so Ruby developers still recognise it

Per model, one line each:

```
class User
  include Model               # B33 field walk, provides the descriptor
  id: Int
  name: String

  def self.all(db: Int)          = Repo.all(db, User.desc)
  def self.find(db: Int, id: Int) = Repo.find(db, User.desc, id)
end
```

These are **concrete** statics on a concrete class, so R3 and R4 never apply. The
boilerplate is real but bounded, honest, and greppable, and B33 can generate it
later without changing the core. **If B77 ever lands, the facade becomes free and
the core does not change.** That is the test of a good factoring.

---

## 3. The schema at compile time

Built on [`B33-codegen-substrate.md`](B33-codegen-substrate.md). What the user
writes:

```
class Todo
  include Model
  id:         Int
  label:      String
  done:       Bool
  created_at: Time
end
```

What B33 synthesizes (its three members, unchanged):

```
def self.field_names -> Array[String]                      # ["id","label","done","created_at"]
def accept[V: FieldVisitor](v: &var V) -> nil              # one visit_* per field
def self.from_fields[R: FieldReader](r: &var R) -> Todo    # one read_* per field
```

What folio adds in `.rx`, with **no new compiler work**:

| Wanted | Delivered by |
|---|---|
| `table_name` | a `Model` default over the class name, overridable. Note it is **not** a field walk, it is B34's `.class.name` territory; until that lands the user writes one line. |
| `columns` | `field_names`, optionally mapped through an overridable `column_name_for` |
| `to_row -> Array[String]` | a `RowWriter` visitor over `accept`: `visit_int` pushes `to_s`, `visit_bool` pushes `"1"`/`"0"`, `visit_time` pushes `iso8601` |
| `from_row(&Json)` | a `JsonRowReader` implementing `FieldReader`, over `from_fields` |
| `column_types` | a `TypeNameVisitor`: `visit_int` pushes `"INTEGER"`, `visit_string` pushes `"TEXT"`. **The SQL type map lives in folio, never in the compiler.** |
| `desc` | a `ModelDesc[Todo]` built from the above, with `decode`/`encode` as closures (R6) |

**The `desc` value is the whole trick.** It converts B33's per-class synthesized
statics, which only concrete code can call, into a value that generic code can
carry.

**B33's nested-field restriction applies** (its E1206): a model field whose type
is itself walkable is refused in v1. For folio that means value objects must be
flattened, which is what a relational row is anyway, so the restriction costs
nothing here.

---

## 4. `Relation[T]`: typed, lazy, and composable today

Proven by R2. `Relation[T]` carries the accumulated SQL fragments and a parallel
`Array[String]` of bound parameters, plus the phantom `T`:

```
struct Relation[T]
  desc:     ModelDesc[T]
  wheres:   Array[String]      # fragments, each with ? placeholders
  params:   Array[String]      # the bound values, positionally
  orders:   Array[String]
  limit:    Option[Int]
  offset:   Option[Int]
end
```

Every combinator returns a new `Relation[T]`, so `T` is preserved by
construction and the type parameter never has to be re-inferred. Nothing runs
until a terminal (`to_a`, `first`, `count`, `each`), which is where the SQL is
assembled and `Db.query_json_params` is called once.

**Naming:** `where` is unavailable (R1). Options: fix the keyword (**B78**, and
there is precedent, `.then` became a contextual keyword method name in pin 973),
or name it something else. **Recommend fixing the keyword**, because
`User.where(...)` is the single most recognisable line in the entire Ruby ORM
surface and a rename is visible in every downstream doc and app forever. Until
B78 lands, folio ships `filter` and aliases it to `where` afterwards.

---

## 5. `where` clauses, and the operators kwargs cannot express

This is where typed ORMs collapse into string fragments, so it gets three tiers
rather than a pretence that one form covers everything.

| Tier | Form | Covers | Status |
|---|---|---|---|
| **1. Bound fragment** | `.filter(&"age > ?", &["18"])` | **everything**: `>`, `LIKE`, `IN`, `OR`, subqueries, functions | works **today** |
| **2. Typed predicate** | `.filter(User.age.gt(18))` | every operator, fully typed, column names checked | needs a column-descriptor per field (a B33 extension) |
| **3. kwargs equality** | `.filter(name: "ada")` | **equality only** | needs B27c, which needs B29a |

**The important honesty:** tier 3 is the one Ruby developers ask for and it is
the **least capable of the three**. `where(name: "ada")` is equality-and-AND by
construction; kwargs syntax has no way to say `>`, `LIKE`, `IN`, or `OR`. Ruby
solves this by falling back to string fragments (`where("age > ?", 18)`), which
is tier 1. So the kwargs form is sugar over the equality case, not the primary
mechanism, and **shipping it first would be building the weakest tier first**.

**Recommended order: tier 1 now, tier 2 next, tier 3 last.** Tier 2 is the real
prize: `User.age.gt(18)` returns a `Predicate` value carrying a fragment and its
params, so composition (`.and`, `.or`, `.not`) is ordinary value composition and
every operator is available with column names checked by the compiler. That is
also what `gems-db.md` §3 calls "the AR-parity endgame"; the probes just move it
from optional to load-bearing.

---

## 6. SQL generation and binding: the safe path is the only path

Global rule 28 forbids interpolating user input into SQL. The design makes the
unsafe path **unspellable** rather than merely discouraged:

1. **A `Relation` never concatenates a value.** Combinators append fragment text
   containing `?` and push values onto `params`. There is no combinator that
   takes a value and puts it into the fragment.
2. **Terminals only ever call `Db.query_json_params` / `Db.exec_params`.** The
   non-`params` ledger entry points are not used by folio at all, except for DDL
   in migrations (§8), which never carries user input.
3. **The escape hatch keeps the guarantee.** `Repo.raw(db, desc, sql, params)`
   exists, and it **has no single-argument form**. There is deliberately no
   `raw(sql)` overload, because that is the shape into which a developer
   interpolates. If you need a literal in raw SQL, you bind it.
4. **Fragments are developer-authored constants, values are bound.** The fragment
   in `.filter(&"age > ?", &["18"])` comes from source text; the value comes from
   `params`. A reviewer can grep for a fragment built with `+` or `#{}` and that
   is the entire audit surface.

**The text-bind limitation, stated plainly.** ledger binds every parameter with
`sqlite3_bind_text` (§1.1). Inserting into an INTEGER column works through
SQLite's affinity conversion, and `WHERE id = ?` generally works for the same
reason, but the behaviour is affinity-dependent rather than exact, and REAL
comparisons are the shakiest case. **Recommendation: add typed binds to ledger**
(`bind_int`, `bind_real`, `bind_null`, `bind_text`) as an early folio
prerequisite, keeping `Array[String]` as the compatibility path. Until then,
document that folio's numeric predicates rely on SQLite affinity.

**`last_insert_rowid` must be added to ledger** before `create` can return a
saved record with its id (§1.1). It is a three-line C function and a one-line
`.rx` declaration.

---

## 7. Connections, transactions, ownership, and thread-per-core

**The handle is an `Int`.** It is `Copy`, so ownership is not the interesting
question; the interesting question is *concurrency*, because a SQLite connection
is not safe to use from two threads at once.

| Question | Answer |
|---|---|
| Who owns a connection | A `Pool` owns an `Array[Int]` of handles plus a guard. Nothing else opens or closes one. |
| How rondo uses it | Thread-per-core, so **one handle checked out per request**, returned on response. This is the pool's whole reason to exist. |
| What a transaction borrows | `transaction(db) do … end` takes the handle by value (it is `Copy`) and brackets `BEGIN` / `COMMIT`, issuing `ROLLBACK` when the block returns `Err`. Nesting uses `SAVEPOINT`. |
| Ambient connection | **No.** The handle is threaded explicitly through every call in v1. A thread-local `Current`-style ambient connection is the classic source of "which connection is this on" bugs, and it fights thread-per-core. Explicit is a small cost paid at every call site and it is always obvious. |
| Panic mid-transaction | `panic!` aborts the process, so the connection dies with it and SQLite rolls back the open transaction on close. **No commit can happen through a panic path.** Recoverable failures are `Result`, which the block helper turns into `ROLLBACK`. This is a real advantage over `ensure`-based cleanup: there is no path where a half-transaction is committed. |
| Pool guard | Needs a mutex. Check `SharedSync` before adding one; `gems-db.md` §9 flags this as the one honest `std` dependency and that still holds. |

---

## 8. Migrations

A migration is a class with `change`, or `up` plus `down`, and the builder is
pure string assembly over `Db.exec`. **This is the lowest-risk part of the whole
line and it should ship early** (it needs no B33, no B77, no kwargs).

**Reversibility (global rule 12)** is enforced structurally rather than by
convention: `change` accepts only operations folio knows how to invert
(`create_table`, `add_column`, `add_index`, `rename_column`). Anything else
(`execute`, `remove_column` with data loss) is **only** available inside `up`,
and defining `up` without `down` is a compile error. So an irreversible migration
cannot be written by accident, only on purpose.

**Schema drift detection.** The model declaration is the single source of truth
(`gems-db.md`'s design note, and it is right, since Ruxen cannot introspect a DB
at compile time). folio therefore offers a runtime check, not a compile-time one:
`Repo.verify_schema(db, desc)` compares `desc.columns` and `desc.column_types`
against SQLite's `PRAGMA table_info`, returning the differences. Call it at boot
in development. A compile-time check is impossible without connecting to a
database at build time, which is a dependency this design refuses to create.

---

## 9. Associations and the N+1 answer

**A typed ORM that makes N+1 easy has failed, so folio makes it structurally
impossible rather than merely discouraged.**

**There is no implicit lazy association accessor.** `user.posts` does not exist
as a property that quietly runs a query. That single decision removes the entire
N+1 class, because N+1 requires a per-record query that looks like a field
access. Instead:

| Need | Form |
|---|---|
| Children of one parent | `Repo.all(db, Post.desc, Post.user_id.eq(user.id))`, visibly a query |
| Children of many parents | `Repo.preload(db, users, Post.desc, &"user_id")` → `Preloaded[User, Post]`, **one** `WHERE user_id IN (?, ?, …)` query, grouped in `.rx` |
| Access after preload | `preloaded.for(user)` → `Array[Post]`, a map lookup, no query |

`Repo.preload` is the two-query batch `gems-db.md` §4 describes, and because the
lazy accessor does not exist, **preloading is not an optimisation you remember,
it is the only way to traverse an association.** The cost is that `has_many` is
less magical than Rails. That is the correct trade for a language with no runtime
reflection, and it is the same reasoning global rule 5 applies to Rails itself,
enforced by construction instead of by review.

Declarations (`has_many :posts`) still earn their place: they generate the
`Post.user_id` column descriptor and the `preload` helper's argument, so the
foreign key is written once.

---

## 10. Not in scope

folio refuses these, each with a reason and a substitute:

| Refused | Why | Substitute |
|---|---|---|
| Dynamic finders (`find_by_name_and_age`) | needs `method_missing`; unbounded names cannot be generated | the typed builder |
| STI (a `type` column selecting a class) | needs runtime type dispatch to pick a decoder | an explicit enum field plus a `match` |
| Polymorphic `belongs_to` | same, over a closed set | one nullable FK per target, or an enum discriminator |
| `default_scope` | invisible global query mutation, a footgun in Rails and worse without reflection to debug it | a named scope you call |
| Implicit lazy associations | the N+1 generator (§9) | `preload` |
| A callback mini-language (`if:`, `unless:`, `on:`) | a DSL the synthesis would have to interpret | write the guard inside the hook |
| Multi-database and sharding | large, and no consumer | one pool per database, chosen at the call site |
| Arbitrary `**opts` in query methods | refused in [`B27-splat-kwargs.md`](B27-splat-kwargs.md) §3 | a typed predicate |
| Connection auto-reconnect and retry | hides real failures behind latency | `Result`, and let the caller retry |

---

## 11. What is undesignable until an enabler lands

**Honest answer: the core is designable and buildable now; the ergonomic surface
is not.**

| Layer | Blocked by | Status |
|---|---|---|
| `Relation[T]`, `Repo`, `ModelDesc`, migrations, transactions, preload | nothing | **designable and buildable today** (R2, R6, and ledger verified running) |
| The `desc` value's contents (`columns`, `to_row`, `from_row`) | **B33** (field walk). Hand-writable until then, which is exactly Phase 0's plan. | designable now |
| `User.all` style facade | nothing (concrete statics work), but it is per-model boilerplate until **B33** generates it | designable now |
| **Generic CRUD inherited from `mixin Model`** | **B77** (static dispatch on an abstract type). This is `gems-db.md`'s Phase 2 as written, and it **cannot be built**. | **undesignable as specified**; re-specified here as `Repo` plus `desc` |
| `.where(...)` by that name | **B78** (`where` is a reserved keyword) | ships as `filter`, renamed later |
| `where(name: "ada")` kwargs form | **B27c**, itself gated on **B29a** | tier 3, last |
| `create` returning the new id | ledger `last_insert_rowid` | small addition, needed early |
| Numeric predicate exactness | ledger typed binds | works via affinity meanwhile |

---

## 12. Revised phasing

QUEUE.md's B54 to B61 phasing came from `gems-db.md`, whose Phase 2 does not
compile. The revised cut points:

| Batch | Was | Now | Change |
|---|---|---|---|
| **B54** | Phase 0, prove the stack, hand-written model | **unchanged, and promoted.** It is no longer a throwaway spike, it **is** the repository core: `Relation[T]`, `Repo`, `ModelDesc`, row decode. Only the hand-written `desc` gets replaced later. | scope grows, importance grows |
| **B54b** *(new)* | — | **ledger additions**: `last_insert_rowid`, typed binds, a transaction helper. Small, additive, unblocks `create`. | new, early |
| **B57** | Phase 3, migrations plus transactions | **unchanged, and moved earlier.** It depends on nothing, and §8's reversibility rule is a real design contribution. | reorder only |
| **B55** | the `Model` field-walk synthesis | **reduced**: it synthesizes the descriptor only, never CRUD. Still B33's second consumer. | scope shrinks |
| **B56** | Phase 2, generic CRUD on `mixin Model` | **re-specified**: `Repo` over `ModelDesc`, no abstract statics. Most of it lands in B54. | fundamentally revised |
| **B58** | associations plus eager loading | **narrowed**: no lazy accessors, preload only (§9) | scope shrinks and sharpens |
| **B59** | validations plus callbacks | unchanged, minus the callback mini-language | minor |
| **B60** | ergonomics | **gains the typed predicate tier** (§5 tier 2), which moves from optional to load-bearing | scope grows |
| **B61** | multi-adapter plus pool | **split**: the pool is needed early (rondo), the Postgres and MySQL FFI is not needed at all yet | pool moves earlier |

**Riskiest, in order:** B56 (it is the one whose original specification was
impossible, so it needs the most re-derivation), then B60's typed predicates
(a B33 extension emitting a descriptor per column, which is new synthesis), then
B61's pool (the one place a `std` concurrency primitive is required).

**Recommended ship order: B57 (migrations) and B54b (ledger) first, both
unblocked and small, then B54 (the repository core), then B55 once B33 lands,
then B58, B59, B60.**

---

## 13. New batches this spec files

- **B77**: static method dispatch through an abstract type (`T.static` in a
  generic body, `Self.static` in a mixin default) fails two different ways
  (R3, R4). Filed as an **enabler**, because landing it would let the
  active-record facade be generic and would simplify B56.
- **B78**: `where` is a reserved keyword and cannot be a method name (R1).
  Precedent: `.then` (pin 973).

---

*Written 2026-07-29 for QUEUE.md batches B54 to B61. Every probe in §1 was run on
that date; R3 and R4 demonstrate that `gems-db.md`'s Phase 2 sketch does not
compile, which is what moved this design from active-record to a repository. No
implementation exists yet.*
