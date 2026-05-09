# Tier 1 — Derive & Macro System

Status: draft
Depends on: tier1_04_drop_copy_clone (pending)
Blocks: stdlib trait coverage, every struct/enum that needs `Debug`/`Eq`/`Hash`

---

## 1. Summary & motivation

Riven's core traits (`Debug`, `Clone`, `Copy`, `Eq`, `PartialEq`, `Hash`, `Default`,
`Ord`, `PartialOrd`) are structural in shape: their implementations are
mechanically derivable from the type definition. Requiring every user to
hand-write them is:

1. **Bug-prone.** Hand-rolled `Hash` + `Eq` often drift out of sync, silently
   breaking hash-map invariants.
2. **A stdlib showstopper.** Every struct in a stdlib hash map, error chain,
   or formatted log must implement these by hand. Collections become painful
   to consume idiomatically.
3. **A blocker for tier1_04 (Drop/Copy/Clone).** The Copy/Clone doc assumes
   `derive Copy, Clone` works end-to-end. Today it is syntactic dead weight
   (see §2). The Copy/Clone semantics document needs this pipeline to exist
   before it can claim conformance.

This document specifies a **compile-time derive system** as the v1 deliverable,
with a clearly-phased path to user-defined declarative macros afterwards.
Full proc-macros (user-defined custom derives) and Crystal/Zig-style
compile-time evaluation are explicit non-goals for v1.

---

## 2. Current state (what exists, what doesn't)

### 2.1 Syntax surface

Two distinct attribute-adjacent syntaxes are already parsed:

1. **Top-level `@[name(arg, ...)]` attributes** — e.g. `@[link("m")]`,
   `@[repr(C)]`. Parsed by `parse_attributes` at
   `crates/riven-core/src/parser/mod.rs:1573-1610`. Produces
   `ast::Attribute { name, args: Vec<String>, span }`
   (`crates/riven-core/src/parser/ast.rs:787-793`).

2. **In-body `derive Trait1, Trait2` clause** inside a `struct ... end`.
   Lexed as a dedicated keyword token `TokenKind::Derive`
   (`crates/riven-core/src/lexer/token.rs:105`, `:327`) and parsed by
   `parse_struct_def` at
   `crates/riven-core/src/parser/mod.rs:832-841` into
   `StructDef.derive_traits: Vec<String>`
   (`crates/riven-core/src/parser/ast.rs:597`).

The tutorial documents the in-body form
(`docs/tutorial/06-classes-and-structs.md:135-141`,
`docs/tutorial/08-traits.md:141-148`,
`docs/tutorial/04-ownership-and-borrowing.md:119-125`). `@[derive(...)]` is
**not** a supported surface form today — the top-level attribute dispatch at
`crates/riven-core/src/parser/mod.rs:473-511` only handles `@[link]` and
`@[repr]`; an `@[derive(...)]` on a struct would be a syntax error because
the `_ => self.error("expected `lib` or `struct` after attribute")` path on
line 508 is reached after the attributes are consumed but `TokenKind::Struct`
is the only struct-opening case, and the `@[repr]` branch stuffs its args
into `derive_traits` as `"repr(C)"`-style strings (see
`parser/mod.rs:497-504`), which is a hack that will conflict with this
proposal.

### 2.2 Propagation through the pipeline

`derive_traits` is faithfully threaded but never **consumed**:

- AST → symbol table: `resolve/mod.rs:375` (pre-pass) and `:811` (full pass)
  copy it into `StructInfo.derive_traits`
  (`resolve/symbols.rs:48`).
- AST → HIR: `resolve/mod.rs:821` copies it into
  `HirStructDef.derive_traits` (`hir/nodes.rs:431`).
- Formatter round-trips it:
  `formatter/format_items.rs:391-402`, `parser/printer.rs:114-118`.

Grepping `derive_traits` under `crates/riven-core/src/typeck`,
`.../mir`, `.../codegen`, `.../borrow_check` returns **zero** matches. The
list is syntactic metadata with no semantic effect.

### 2.3 Trait registration

The compiler pre-registers `Copy`, `Clone`, `Debug`, `Drop` as built-in
traits with empty/minimal method sets
(`resolve/mod.rs:138-170`, lines 147-150):

```
("Copy", vec![]),
("Clone", vec!["clone"]),
("Debug", vec![]),
("Drop", vec!["drop"]),
```

`Eq`, `PartialEq`, `Hash`, `Default`, `Ord`, `PartialOrd` are **not**
registered as built-in traits. Structural method resolution in
`typeck/traits.rs:81-120` would fail for them today.

### 2.4 `is_copy` does not consult derives

`Ty::is_copy` (`hir/types.rs:189-221`) returns `false` for `Ty::Struct { .. }`
unconditionally. `derive Copy` has no effect on ownership semantics in
the borrow checker — today it is pure documentation.

### 2.5 The format-string interpolation template

The review compared derive expansion to the existing format-string
interpolation expansion. Worth noting: string interpolation is **not**
expanded into desugared AST/HIR. It flows as a first-class construct:

- Lexer produces `TokenKind::InterpolatedString(Vec<StringPart>)`
  (`lexer/mod.rs:243-295`, `lexer/token.rs`).
- Parser lifts it to `ExprKind::InterpolatedString(Vec<StringPart>)`
  (`parser/expr.rs:214-221`, `parser/ast.rs:216`).
- HIR carries `HirExprKind::Interpolation { parts }`
  (`hir/nodes.rs:223-247`).
- MIR lowering in `mir/lower.rs:1232` and the helper
  `lower_interpolation` at `mir/lower.rs:2024-2128` emits the concatenation
  calls (`riven_string_concat`, `riven_int_to_string`, etc.) directly into
  MIR.

Implication: **Riven has no precedent for an AST/HIR rewrite pass.** Every
"expansion" today is a first-class node lowered straight to MIR. The
derive system is the first feature that genuinely needs either a pre-resolve
AST synthesis pass or a post-HIR impl-synthesis pass. This doc picks one in
§5 and justifies it.

---

## 3. Goals & non-goals

### Goals (v1)

- A working `@[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default, Ord, PartialOrd)]`
  surface syntax that produces real impls — i.e. the derive list actually
  drives codegen, `is_copy` consults it, `puts` can print any
  `@[derive(Debug)]` struct.
- Unify the two current entry points (`@[derive(...)]` and in-body
  `derive T1, T2`) on one canonical form. Both should work; one becomes the
  idiomatic default.
- Coverage: structs (named + tuple + unit), enums (unit + tuple + struct
  variants), generics (with correct bound propagation: `T: Debug` for every
  type parameter referenced in a derived `Debug` impl).
- Actionable diagnostics: `derive(Copy)` on a struct with a non-Copy field
  points at the offending field.
- Hooks so tier1_04 (Drop/Copy/Clone) can rely on derived `Copy`/`Clone`
  impls existing after this phase.

### Non-goals (v1)

- **User-defined derives** (custom `derive` macros). Deferred to a later
  tier.
- **Declarative macros** (`macro_rules!`-style). Sketched in §7, phased in
  §11 as 5d.
- **Function-like macros** (`foo!(...)` call syntax).
- **Attribute-like macros** beyond the fixed set in §2.1.
- **Compile-time evaluation** (Zig `comptime`, Nim AST macros, Crystal
  `macro`). Out of scope for tier 1.
- **Hygiene beyond the one-level "generated identifiers don't collide with
  user names" rule** (see §7.3).

### Non-goals with soft yes later

- Attribute helpers on derived fields (e.g.
  `@[serde(rename = "foo")]`). The attribute AST already has room for named
  args; we carve out space without implementing it.

---

## 4. Built-in derive catalog

All nine derives are **compiler-built-in**: hard-coded in Rust inside
`riven-core`. Rationale in §5.4. Spec for each is given as: what it
generates, what bounds it adds, what errors are raised.

### 4.1 `Debug`

**Generates:** `impl[GP] Debug for T[GP]` with method
`def fmt(&self) -> String`. The returned string is the struct/enum printed
in Riven source-ish form:

- Named struct `Point { x: 1, y: 2 }` →  `"Point { x: 1, y: 2 }"`
- Tuple struct `Wrap(42)` → `"Wrap(42)"`
- Unit struct `E` → `"E"`
- Enum unit variant → `"Variant"`
- Enum tuple variant → `"Variant(1, 2)"`
- Enum struct variant → `"Variant { a: 1, b: 2 }"`

**Bounds added:** `T_i: Debug` for each type parameter `T_i` that appears
in any field type.

**Implementation note:** emits HIR using
`HirExprKind::Interpolation` (§2.5) directly — no new runtime function
needed. Each field is `field.fmt()` spliced into the interpolated string.

**Errors:** None inherent — all fields getting `Debug` is a bound on the
generated impl, enforced by typeck at impl definition time.

### 4.2 `Clone`

**Generates:** `impl[GP] Clone for T[GP]` with
`def clone(&self) -> Self`. For each field, emits
`self.field.clone()`. For an enum, a `match` over variants reconstructing
each one.

**Bounds added:** `T_i: Clone` for each type parameter in a field type.

**Errors:** None at definition site — missing `Clone` on a field type is a
trait-bound failure inside the generated body, reported with a span that
points at the field whose type is missing `Clone` (§9.4).

### 4.3 `Copy`

**Generates:** `impl[GP] Copy for T[GP]` — the trait has no methods
(`resolve/mod.rs:147`). Also updates `Ty::is_copy` to return `true` for
`Ty::Struct { name, .. }` / `Ty::Enum { name, .. }` when the symbol table
records a derived `Copy` impl for `name` (§5.5).

**Bounds added:** `T_i: Copy` for every type parameter referenced in a field.

**Errors:**
- **Non-Copy field in struct.** Emit `E0601: cannot derive Copy on struct
  `Foo` because field `bar: Bar` is not Copy`. Span: the offending field's
  `TypeExpr`. Note: recurse on generic args when the field type is
  parameterized.
- **Copy without Clone.** Rust requires `Copy: Clone`. Riven should follow
  suit: emit `E0602: deriving Copy also requires Clone` and suggest
  `@[derive(Copy, Clone)]`. (Alternative: auto-add Clone; rejected for
  explicitness.)
- **Derived on a `class`.** Classes are heap/reference types; emit
  `E0603: Copy cannot be derived on classes; use a struct`.

### 4.4 `PartialEq`

**Generates:** `impl[GP] PartialEq for T[GP]` with
`def eq(&self, other: &Self) -> Bool`. Struct: conjunction of
`self.f_i == other.f_i`. Enum: `match` on `(&self, &other)` returning `true`
when both sides are the same variant with all corresponding fields equal.

**Bounds added:** `T_i: PartialEq` for every type parameter in a field.

**Trait registration:** adds `PartialEq` with required method `eq` to
the built-in trait list at `resolve/mod.rs:139-151`.

### 4.5 `Eq`

**Generates:** `impl[GP] Eq for T[GP]` — marker trait, no methods.
`Eq: PartialEq` is required.

**Errors:** `E0604: deriving Eq also requires PartialEq`.

### 4.6 `Hash`

**Generates:** `impl[GP] Hash for T[GP]` with
`def hash(&self, hasher: &mut Hasher) -> Unit`. For each field emits
`self.field.hash(hasher)`. For enums, also hashes the variant discriminant
first.

**Bounds added:** `T_i: Hash`.

**Trait registration:** adds `Hash` as a built-in trait (currently the
token `Hash` is already a built-in **type constructor** for hash maps at
`resolve/mod.rs:200`; the trait needs a disambiguated name or the type
constructor needs a rename. See §12 Open questions.).

### 4.7 `Default`

**Generates:** `impl[GP] Default for T[GP]` with
`def default() -> Self`. Struct: `Self { f_i: Default::default(), ... }`.
Enum: requires exactly one variant annotated `@[default]`; otherwise error.
Unit variant defaults to itself.

**Bounds added:** `T_i: Default`.

**Errors:** `E0605: cannot derive Default on enum Foo without a #[default]
variant`. For structs with no fields, trivially emit `Self {}` / unit.

### 4.8 `Ord`

**Generates:** `impl[GP] Ord for T[GP]` with
`def cmp(&self, other: &Self) -> Ordering`. Struct: lexicographic over
fields in declaration order. Enum: by variant index first, then
lexicographic over payload.

**Bounds added:** `T_i: Ord`. Requires `Eq + PartialOrd`.

**Errors:** `E0606: deriving Ord also requires Eq and PartialOrd`.

### 4.9 `PartialOrd`

Same as `Ord` but returns `Option[Ordering]` and only requires
`PartialEq`. Delegates to `PartialOrd::partial_cmp` on each field.

### 4.10 Interactions table

| derive       | requires also | adds `T: _` bound | affects `is_copy`? | marker? |
|--------------|---------------|-------------------|--------------------|---------|
| `Debug`      | —             | `Debug`           | no                 | no      |
| `Clone`      | —             | `Clone`           | no                 | no      |
| `Copy`       | `Clone`       | `Copy`            | **yes**            | yes     |
| `PartialEq`  | —             | `PartialEq`       | no                 | no      |
| `Eq`         | `PartialEq`   | `Eq`              | no                 | yes     |
| `Hash`       | —             | `Hash`            | no                 | no      |
| `Default`    | —             | `Default`         | no                 | no      |
| `PartialOrd` | `PartialEq`   | `PartialOrd`      | no                 | no      |
| `Ord`        | `Eq`,`PartialOrd` | `Ord`         | no                 | yes     |

---

## 5. Derive expansion pipeline

### 5.1 Where expansion runs

**Decision: expand after resolve, during HIR construction — produce
synthetic `HirImplBlock`s appended to the program.**

The candidates considered:

1. **Pre-resolve (AST → AST rewrite).** Generate
   `ast::ImplBlock` nodes before name resolution. Pros: the derived impls
   go through the normal resolver and type checker with no special casing;
   integrates cleanly with LSP (goto-def on `clone` resolves to the
   synthesized method). Cons: must spell method bodies in AST
   (`ast::Expr`), which means constructing `Path`s, `FieldAccess`es, and
   `BinaryOp`s programmatically with synthetic spans; easy to get the
   shape wrong and hit confusing "undefined variable `self`" errors.

2. **Post-resolve / during HIR lowering** (chosen). The derive pass runs
   **after** `resolve_struct` / `resolve_enum` have populated the
   `SymbolTable` with field `DefId`s. It directly constructs
   `HirImplBlock` nodes whose method bodies are `HirExpr`s with
   pre-resolved `DefId`s and types. Pros: no fake identifiers; types are
   known; we skip a round-trip through resolve. Cons: introduces a small
   amount of duplication — e.g. the equivalent of resolving a
   method call to `DefId`.

3. **Post-HIR, in MIR.** Rejected. MIR is function-scoped, not
   item-scoped; synthesizing impl blocks there means bypassing trait
   resolution and the borrow checker, both of which need to see derived
   impls as real impls.

### 5.2 Pipeline placement

```
parse   → ast::Program
resolve → HirProgram (fields resolved, impl_blocks from source only)
        ↓
  DERIVE EXPANSION  ← new phase (§5.3)
        ↓
typeck  → inferred HirProgram (now sees derived impls too)
borrow_check
mir lowering
codegen
```

Concretely: `typeck::type_check` at
`crates/riven-core/src/typeck/mod.rs:37-69` currently runs
`Resolver::resolve` then `TraitResolver::collect_impls`. Insert a new
`DeriveExpander::expand(&mut program, &mut symbols)` between those two
lines. It **must** run before `TraitResolver::collect_impls` so collected
impls include the derived ones.

### 5.3 `DeriveExpander` module

New file: `crates/riven-core/src/derive/mod.rs` (and submodules per derive
kind). Public API:

```rust
pub struct DeriveExpander<'a> {
    symbols: &'a mut SymbolTable,
    type_context: &'a mut TypeContext,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> DeriveExpander<'a> {
    pub fn expand(&mut self, program: &mut HirProgram);
}
```

Walk every `HirItem::Struct` and `HirItem::Enum`. For each name in
`derive_traits`, dispatch to a per-derive generator:

```rust
fn expand_debug_struct(&mut self, s: &HirStructDef) -> HirImplBlock;
fn expand_clone_struct(&mut self, s: &HirStructDef) -> HirImplBlock;
// ... etc
fn expand_debug_enum(&mut self, e: &HirEnumDef) -> HirImplBlock;
// ... etc
```

Generators return `HirImplBlock`s which are appended to `program.items`.

### 5.4 Why compiler-built-in (not written in Riven)

The tension is: a self-hosting language wants these in the surface
language. But:

- **Bootstrapping.** Riven's stdlib depends on `Debug`/`Clone`. If the
  derives are written in a Riven macro, we have a circular dependency
  at stdlib build time.
- **Simplicity.** Hard-coding nine derives in ~800 lines of Rust is small.
  A general macro engine powerful enough to express these is
  significantly larger.
- **Error quality.** Native-Rust generators have full access to the
  compiler's diagnostic infrastructure and can point at precise field
  spans; user-written derives eventually need this too but not in v1.
- **Future-proof.** Once a declarative or proc-macro system lands
  (§7), the nine built-ins can be progressively ported to Riven source
  without breaking the surface syntax.

### 5.5 Updating `is_copy`

`Ty::is_copy` (`hir/types.rs:189-221`) must learn to look up user types.
Since `Ty::Struct { name, .. }` / `Ty::Enum { name, .. }` carry only names,
either:

- Extend `Ty` variants with a `is_copy` cache bit, populated at resolve
  time from `derive_traits`. Awkward: duplicates information.
- Give `is_copy` a `&SymbolTable` parameter. Touches every call site.
- **Chosen:** keep `is_copy()` as the fast path (returns `false` for user
  structs/enums conservatively) and add
  `is_copy_with(&self, symbols: &SymbolTable) -> bool` that consults
  `StructInfo.derive_traits`. Borrow check and move analysis switch to the
  `_with` form; other sites can stay on the cheap one.

The `DeriveExpander` also marks structs/enums in the symbol table:

```rust
// resolve/symbols.rs additions
pub struct StructInfo {
    // ...
    pub derive_traits: Vec<String>,   // already exists
    pub derived_copy: bool,           // new — set by DeriveExpander
    pub derived_clone: bool,          // new — required by Copy
    // ... one bool per derive, or an EnumSet/bitflag
}
```

### 5.6 Generic bound propagation

For a generic type `struct Pair[T, U] { a: T, b: U } derive Debug`, the
generated impl must carry `where T: Debug, U: Debug`. The expander walks
each field type and collects every type-parameter name referenced. Each
reference → one bound. Transitive: if the field type is
`Vec[T]`, we still only need `T: Debug` because `Vec`'s `Debug` impl is
itself `impl[T: Debug] Debug for Vec[T]`.

The generated `HirImplBlock.generic_params` replicates the type's
`generic_params` with bounds augmented.

### 5.7 Errors surfaced at expansion time

`DeriveExpander` catches **definition-site** errors — the ones listed per
derive in §4. Trait-bound violations inside a generated body (e.g. Clone
derives a body that calls `field.clone()` but the field type doesn't
implement Clone) are **not** caught by the expander; they surface during
type-check of the generated HIR. That is correct because: (a) the
expander doesn't have full trait resolution yet, and (b) the resulting
error points at the generated impl with a note that the body was
auto-derived — see §9.4 for the span strategy.

---

## 6. Attribute syntax

### 6.1 Canonical form

```
@[derive(Debug, Clone, Copy)]
struct Point
  x: Float
  y: Float
end
```

Decision: `@[derive(...)]` is the canonical form. The existing
`@[derive(...)]` dispatch gap in the parser (§2.1) **must be closed**: the
top-level attribute handler at
`crates/riven-core/src/parser/mod.rs:473-511` needs a `"derive"` arm that
stores the args on `StructDef.derive_traits` (similar to how `"repr"` is
handled on line 499-503, minus the `repr()` stringification hack).

### 6.2 Backward-compat for the in-body `derive` keyword

Two options:

- **(A) Keep both.** `@[derive(...)]` above the struct **and** `derive ..`
  inside the body merge into the same `derive_traits`. Tutorial uses the
  `@[derive]` form going forward; the in-body form stays as a deprecated
  alias for one release.
- **(B) Remove in-body.** Breaking change; touches `TokenKind::Derive`,
  the struct-parsing loop, all three tutorial files, and the formatter.

Recommendation: **(A)**. Zero breakage, cost is a line of merge logic.
The formatter emits the `@[derive(...)]` form on reformat, migrating
sources organically.

### 6.3 Composition

`@[derive(A, B)]` and `@[derive(C)]` on the same item stack additively.
Duplicates are silently deduplicated. Order is irrelevant.

### 6.4 Attribute arg grammar

`parse_attr_arg` (`parser/mod.rs:1613-1633`) currently accepts string
literals, identifiers, and type identifiers. For derive we only need
type identifiers (trait names). Future derives may need key-value args
(e.g. `@[serde(rename = "foo")]`) — that is a grammar extension we defer.
Document in §12 as an open question.

### 6.5 Applicable targets

| attribute   | struct | enum  | class | trait | fn  |
|-------------|--------|-------|-------|-------|-----|
| `derive`    | yes    | yes   | no*   | no    | no  |
| `repr`      | yes    | no    | no    | no    | no  |
| `link`      | no     | no    | no    | no    | lib |

\*Classes can have `@[derive(Clone)]` later (needs recursive Clone over a
heap pointer — fine) but **not** `Copy` (§4.3). Out of scope for v1.

Attribute application on an invalid target is a parse-level error:
`E0607: @[derive] cannot be applied to a trait`.

---

## 7. User-defined macros (phased later)

This is the 5d milestone, not v1. Sketch only.

### 7.1 Declarative macros (preferred first step)

Borrowing from Rust's `macro_rules!` but rendered in Riven keyword style:

```
macro vec!
  () => { Vec.new }
  ($x:expr) => {
    let v = Vec.new
    v.push($x)
    v
  end }
  ($x:expr, $($rest:expr),+) => {
    let v = vec!($($rest),+)
    v.push($x)
    v
  end }
end
```

- Fragment specifiers: `expr`, `ident`, `ty`, `pat`, `stmt`, `tt` (token
  tree), `literal`.
- Expansion target: AST. Matches tokens, substitutes into an AST
  template, re-parses.
- **Token tree layer.** Riven does not today have a public
  `TokenStream`/`TokenTree` representation. The lexer emits
  `Vec<Token>` which the parser consumes. For macros we'd grow a
  `TokenTree` enum (`Group`, `Ident`, `Punct`, `Literal`) — add this to
  `riven-core/src/macros/` when 5d lands.

### 7.2 Function-like, attribute-like, custom derives

All three are compile-time Riven code that runs inside the compiler. The
Rust approach (separate `proc_macro` crates compiled separately, loaded
as shared libraries) is a significant engineering investment. Crystal's
approach (macros are interpreted in a mini-AST-level evaluator built into
the compiler) is cheaper. Riven is unlikely to need proc-macros for
years; leave the door open by making the declarative macro engine the
first stop, and revisit when concrete demand appears.

### 7.3 Hygiene

For v1 built-in derives the rule is simple: **synthesized identifiers use
names that the lexer rejects for user code**. Prefix generated locals
with `$riven$` (dollar sign is not a valid identifier char in the lexer).
Generator never introduces a free identifier that could shadow a user
binding.

For 5d declarative macros, Rust-style span-based hygiene is the correct
long-term answer but is complex. The intermediate step is "mixed-site"
hygiene: identifiers written inside a macro's template are resolved in
the macro's definition environment; identifiers matched via `$var:ident`
are resolved in the call site's environment. This is what `macro_rules!`
actually provides and is good enough for 95% of uses.

### 7.4 Public API surface

For v1: there is **no** user-facing macro/derive API. `riven-core` has no
`TokenStream` in its public surface. Users cannot import from
`riven::macros::*` because no such module exists.

For 5d: introduce `riven-core::macros` with `TokenStream`, `TokenTree`,
`Span`, `Spanned<T>`. Intentionally lean; proc-macro-style
`quote!`/`parse_macro_input!` deferred.

---

## 8. Interaction with other features

### 8.1 Drop / Copy / Clone (tier1_04)

The derive pipeline produces the impls that tier1_04 assumes exist. Order
of landing: **this doc first** (at least the `Copy`/`Clone` slice, phase 5a
+ 5b), then tier1_04 can wire `is_copy_with` and move analysis against
derived impls. If 5b slips, tier1_04 must stub `is_copy` for
user structs — tolerable but regressive.

### 8.2 Stdlib circular dependency

`Debug`, `Clone`, etc. are declared in stdlib. Derives emit impls that
reference these trait names. If stdlib itself has structs that need
`@[derive(Debug)]`, the derive expander runs on stdlib compilation and
references a trait that is defined **in the same compilation unit**. This
is fine because:

- Resolve pre-pass registers all top-level trait names before the expander
  runs (`resolve/mod.rs:138-170` already does this for the built-in list).
- The generated impl's method signatures are fully concrete HIR — no
  forward name lookup required during typeck.

For stdlib trait definitions themselves (the definition of `trait Clone
... end` in stdlib source), we do **not** derive; those are hand-written.
The built-in trait list in `resolve/mod.rs` is a bootstrap shim that will
eventually be removed once stdlib is loaded as a real dependency.

### 8.3 LSP / `riven-ide`

Derived impls must be discoverable by hover and goto-definition. The
expander attaches a `Span` to each generated item pointing at the
original `@[derive(TraitName)]` attribute's span — not a synthesized
span. `riven-ide` then treats goto-def on a call to `.clone()` on a
derived struct as "jumps to the `@[derive(Clone)]` attribute on the
struct definition", which is the right UX.

If we later want goto-def to instead show a synthesized code view, add a
`Synthesized { origin_attr_span, trait_name }` span variant; not needed
for v1.

### 8.4 Formatter

`formatter/format_items.rs:391-402` today emits the in-body `derive X, Y`
form. It must be taught the `@[derive(...)]` form and emit that when
either syntax is input. See §6.2.

### 8.5 Trait resolution

`typeck/traits.rs` collects impls via `collect_impls`. Derived impls
appended in §5.3 flow through unchanged. Structural satisfaction
(`TraitSatisfaction::Structural` at `traits.rs:20-22`) continues to work
for static-dispatch cases; nominal satisfaction now covers derived
impls too.

---

## 9. Implementation plan

Each step is a separately reviewable PR.

### 9.1 Step 1 — Close the parser gap

- Add `"derive"` arm at `parser/mod.rs:477-511`. Args go into
  `StructDef.derive_traits`.
- Extend `parse_attributes` dispatch to also accept `@[derive]` on
  `enum` (currently `enum` isn't even in the match — see
  `parser/mod.rs:495` only handles `Lib` and `Struct`). Add enum support
  by extending `EnumDef` with `derive_traits: Vec<String>` (mirror of
  `StructDef.derive_traits`).
- Remove the `repr(C)` string-stuffing hack at `parser/mod.rs:499-503`;
  move `repr` to its own field on `StructDef`.
- Unit tests: parse `@[derive(Debug)]`, `@[derive(A, B, C)]`, both forms
  together; error for `@[derive]` on `fn` or `trait`.

### 9.2 Step 2 — Register missing built-in traits

- `resolve/mod.rs:138-151`: append `PartialEq`, `Eq`, `Hash` (as trait,
  distinct from the `Hash` **type constructor** at line 200 — see §4.6 /
  §12 about the naming collision), `Default`, `Ord`, `PartialOrd`.
- Each with correct required methods (`eq`, `hash`, `default`, `cmp`,
  `partial_cmp`).

### 9.3 Step 3 — Scaffold `riven-core::derive` module

- Create `crates/riven-core/src/derive/mod.rs` with the struct layout
  from §5.3.
- Wire into `typeck::type_check` between phase 1 (resolve) and phase 2
  (collect_impls).
- No generators yet — just the walking shell.
- Stub diagnostic: every derive name produces `E9999: derive of `X` not
  yet implemented`. Each subsequent step drops one entry from this stub
  list.

### 9.4 Step 4 — Per-derive generators (phased)

Order by dependency:

- **5a:** `Debug`, `Clone`. Unblocks printing and stdlib plumbing.
- **5b:** `Copy`, `PartialEq`. `Copy` requires Clone (already there).
  Unblocks tier1_04. Also update `Ty::is_copy_with` and symbol-table
  `derived_copy` flag.
- **5c:** `Eq`, `Hash`, `Default`. `Eq` requires PartialEq (5b). `Hash`
  pairs naturally with `Eq` for hash-map usage.
- **5c':** `Ord`, `PartialOrd`.

Each generator:

1. Walk fields / variants, build HIR expression trees directly.
2. Construct an `HirImplBlock` with
   `trait_ref = Some(TraitRef { name: "Debug", generic_args: vec![] })`
   and `target_ty = Ty::Struct { name, generic_args }`.
3. Append to `program.items`.
4. Every span inside the generated impl uses the original derive
   attribute's span (§8.3). Every synthesized identifier is prefixed
   `$riven$` (§7.3).

### 9.5 Step 5 — Error reporting

Diagnostic codes: E0601–E0609 (reserved block). Template:

```
error[E0601]: cannot derive Copy on struct `Foo`
  --> foo.rvn:3:3
3 |   inner: Vec[Int]
  |   ^^^^^^^^^^^^^^^ field type `Vec[Int]` is not Copy
  |
note: Copy was requested here
  --> foo.rvn:1:1
1 | @[derive(Copy)]
  | ^^^^^^^^^^^^^^^
```

Two spans always: the offending field (or variant), and the
`@[derive(...)]` origin.

For errors that surface in generated bodies (§5.7), the
type-check pipeline must detect "this impl came from a derive" via a
`HirImplBlock.origin: ImplOrigin` field (new — `Origin::Source` vs
`Origin::Derived { attr_span, trait_name }`). Type errors inside derived
bodies then reformat as "deriving `Clone` on `Foo` failed because …".

### 9.6 Step 6 — Borrow check integration

- `is_copy_with` (§5.5) in `hir/types.rs`.
- `borrow_check/moves.rs:51,59` switch from `Ty::is_copy` to
  `is_copy_with(&symbols)`.
- Regression tests: struct with `@[derive(Copy, Clone)]` survives a
  re-use after assignment; struct without derive still errors.

### 9.7 Step 7 — Formatter update

Teach `formatter/format_items.rs:385-402` and `formatter/format_items.rs`
(new enum branch) to emit `@[derive(...)]` above the struct/enum. Leave
the in-body emit path only for sources where the input used it, or
default all emits to `@[derive(...)]` (cleaner; adopt this).

### 9.8 Step 8 — Documentation

- `docs/tutorial/08-traits.md:139-149`: rewrite using `@[derive(...)]`.
- Same for `docs/tutorial/06-classes-and-structs.md:131-145` and
  `docs/tutorial/04-ownership-and-borrowing.md:119-125`.

---

## 10. Test matrix

For each combination cell, a fixture under
`crates/riven-core/tests/fixtures/derive/` and an integration test under
`crates/riven-core/tests/derive_*.rs` asserting
(1) compiles, (2) produces expected output or expected diagnostic.

### 10.1 Shape axis

| Shape                           | Example                                   |
|---------------------------------|-------------------------------------------|
| Named struct, no generics       | `struct P { x: Int, y: Int }`             |
| Named struct, generics          | `struct Pair[T, U] { a: T, b: U }`        |
| Tuple struct                    | `struct Wrap(Int)`                        |
| Unit struct                     | `struct Unit end`                         |
| Enum, all unit variants         | `enum Color { Red, Green, Blue }`         |
| Enum, tuple variants            | `enum E { A(Int), B(Str, Bool) }`         |
| Enum, struct variants           | `enum E { A { x: Int } }`                 |
| Enum, mixed                     | `enum Shape { Unit, P(Int), R{w:Int,h:Int} }` |
| Recursive type                  | `struct List[T] { head: T, tail: Option[Box[List[T]]] }` |

### 10.2 Derive axis

Every combination of the nine derives, at minimum the diagonal plus the
common bundles: `(Debug)`, `(Clone)`, `(Copy, Clone)`,
`(Debug, Clone)`, `(Eq, PartialEq, Hash)`, `(Ord, Eq, PartialOrd,
PartialEq)`, all nine together.

### 10.3 Error axis

- `@[derive(Copy)]` on struct with `Vec[Int]` field → E0601, points at
  the field.
- `@[derive(Copy)]` without `Clone` → E0602.
- `@[derive(Copy)]` on a class → E0603.
- `@[derive(Eq)]` without `PartialEq` → E0604.
- `@[derive(Default)]` on enum without `@[default]` variant → E0605.
- `@[derive(Ord)]` without `Eq, PartialOrd` → E0606.
- `@[derive]` on `fn foo` → E0607.
- `@[derive(NotARealTrait)]` → E0608 (unknown derive).
- Generic `@[derive(Clone)]` where a type-param field is not Clone →
  reported at use site (not definition).

### 10.4 Attribute ordering

- `@[derive(Debug)] @[derive(Clone)]` stacks.
- `@[derive(Debug)] @[repr(C)]` does not interfere.
- Mixed in-body `derive X` **and** `@[derive(Y)]` merges (§6.2).

### 10.5 Formatter roundtrip

Every fixture feeds into `rivenc fmt` and the output must re-parse to
the same HIR.

### 10.6 LSP / IDE

`riven-ide` hover on a `.clone()` call for a derived struct shows the
generated impl signature; goto-def lands on the `@[derive(Clone)]`
attribute (§8.3).

---

## 11. Phasing

| Phase | Content                                       | Blocks                         |
|-------|-----------------------------------------------|--------------------------------|
| 5a    | Parser gap closed (§9.1), trait registration (§9.2), `DeriveExpander` scaffold (§9.3), `Debug`+`Clone` generators (§9.4), basic error reporting (§9.5), formatter update (§9.7), docs (§9.8) | printing/logging any struct |
| 5b    | `Copy`+`PartialEq`, `is_copy_with` + borrow check integration (§9.6) | tier1_04 Drop/Copy/Clone doc |
| 5c    | `Eq`+`Hash`+`Default`                         | stdlib hash-map usage          |
| 5c'   | `Ord`+`PartialOrd`                            | `sort`, `BTreeMap`             |
| 5d    | Declarative macros (future; §7)               | user-ext macros, custom derive |
| 5e    | Proc-macros / custom derive (future)          | power-user ergonomics          |

5a and 5b are committed for tier 1. 5c/5c' strongly recommended as part of
tier 1 because the stdlib is unusable without them. 5d and 5e are
explicitly deferred.

---

## 12. Open questions & risks

### 12.1 `Hash` naming collision

`Hash` is both:

- a built-in type constructor for hash maps
  (`resolve/mod.rs:200`), and
- the trait we want to register for `@[derive(Hash)]`.

In Rust these are `std::collections::HashMap` and `std::hash::Hash`, distinct by
path. Riven doesn't have modules in the symbol table that disambiguate
them in the same way. Options:

- Rename the hash-map type to `HashMap` (breaks existing code).
- Rename the trait to `Hashable` (fits the existing `Displayable`,
  `Comparable` pattern at `resolve/mod.rs:140-143`). **Recommended.** Then
  `@[derive(Hashable)]` emits an `impl Hashable for T` block. The type
  constructor `Hash[K, V]` is untouched.

If we go with `Hashable`: document the naming convention in a cross-cutting
ADR — all stdlib derive-able traits end in `-able` (`Displayable`,
`Hashable`, `Clonable`?) or none do. Currently `Debug`, `Clone`, `Copy`,
`Eq`, `Ord` are fine by both Rust and English. Pick and commit.

### 12.2 Attribute args beyond bare type names

`@[derive(Debug)]` uses `parse_attr_arg` which accepts string literals
and identifiers. Future `@[serde(rename = "foo", default)]` needs
key-value pairs and richer expressions. Not needed for v1 but the AST
(`Attribute { args: Vec<String> }` at `ast.rs:789-793`) is too weak.
Change to `args: Vec<AttrArg>` where `AttrArg` supports
`Value(Expr)` / `KeyValue(String, Expr)` **now**, even if the grammar
doesn't parse them yet. Cheap future-proofing.

### 12.3 Orphan rule for derived impls

Rust's orphan rule forbids `impl ForeignTrait for ForeignType` except
when one is local. Derives sidestep this because the type is always
local (you can only derive on a type you own). Nothing to decide, but
document explicitly: v1 does not support deriving a foreign trait for a
foreign type because no mechanism exists for it.

### 12.4 Generic bound explosion

`struct Tree[T] { left: Box[Tree[T]], right: Box[Tree[T]], val: T }` with
`@[derive(Debug)]` produces an impl with bound `T: Debug`. Rust's
current heuristic adds `T: Debug` for every type parameter that appears
"anywhere" in a field. Mostly right but over-bounded in phantom-data
cases — we don't have phantom data in Riven yet, so the simple heuristic
holds. Revisit when `PhantomData` lands.

### 12.5 Debug output format

Is `Point { x: 1, y: 2 }` the right default, or `Point(1, 2)`, or JSON?
Rust picked braced-struct form. Committing to that because tooling-friendly,
but the decision is worth one explicit line in the final doc.

### 12.6 Performance

Deriving nine traits on every struct across a large program is a
non-trivial HIR volume increase. Measure at 5a landing: count HIR item
count before/after. If the expander materially hurts incremental rebuilds
in `rivenc`'s content-addressed cache, adopt lazy generation (derive on
demand when a trait bound requires it). Almost certainly a premature
optimization; flag for vigilance.

### 12.7 Risk: scope creep into full macros

The fastest path to a maintainable Riven is "builtin derives only, no
user-extensibility until later". The temptation to ship a minimally-
capable `macro` keyword alongside this work should be resisted —
v1 = nine hard-coded derives, full stop. The 5d phase exists to make the
staging explicit.

### 12.8 Risk: hygiene debt

The `$riven$` identifier prefix works for v1 but is not real hygiene.
Any future macro work that introduces user identifiers will need span-
based hygiene. The refactor from prefix-based to span-based is
non-trivial but bounded (touches resolve/name lookup only). Budget a
week for it when 5d lands; do not block v1 on it.
