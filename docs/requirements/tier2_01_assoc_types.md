# Tier 2.01 — Associated Types

Status: Draft (requirements)
Owner: compiler
Depends on: nothing in tier-2; consumes tier-1 drop/copy/clone for
            correct field-drop of projected types
Blocks: GATs (doc 05), stdlib Iterator (tier-1 doc 01 phase 1a)

## 1. Summary & Motivation

An *associated type* is a type-level member of a trait that each impl
resolves to a concrete type. The canonical example is `Iterator::Item`:

```riven
trait Iterator
  type Item
  def mut next -> Option[Self.Item]
end

impl Iterator for Vec[Int]
  type Item = Int
  def mut next -> Option[Int]
    self.pop
  end
end
```

Without associated types, the stdlib cannot express `Iterator` — the
trait either has to be generic over `Item` (`trait Iterator[Item]`), in
which case `Vec.iter` returns `impl Iterator[&Int]` but the *type* of
`.next` forgets that binding and `.collect` cannot recover it, or it
has to fix `Item` to a single type (`Int` today — see tier-1 doc 01
§2.1: the runtime is hardcoded int64). Every mainstream language with a
trait/protocol system and a generic iterator (Rust, Swift, Haskell,
Scala) uses an associated type, not a trait type parameter, for this
reason.

Beyond `Iterator`, the same mechanism is wanted for:

- `FromIterator::Item` (mirror of `Iterator::Item`),
- `IntoIterator::IntoIter` (the iterator type, not the item),
- `Hasher::Output` (a future hash-abstraction trait — tier-1 doc 01
  phase 1c),
- `Future::Output` (blocks tier-1 doc 03 async),
- `Add::Output`, `Mul::Output` (operator traits — not in tier 1 or 2
  but mentioned to show the pattern),
- `Deref::Target` (if/when `Deref`-style auto-dereferencing ships).

The tutorial already describes the feature
(`docs/tutorial/08-traits.md:27-33`), and the fixture sample program
in tier-1 doc 01 (stdlib) implicitly depends on it (Vec chains that
compile to `riven_noop_passthrough`).

This doc specifies syntax, resolution, projection (the type-level
operation that turns `Vec[Int]::Item` into `Int`), type-check rules,
and the MIR / codegen story post-monomorphization.

## 2. Current State

### 2.1 Parsing and resolve already accept the surface syntax

`TraitItem::AssocType { name: String, span: Span }` is defined at
`parser/ast.rs:644-648` and parsed at `parser/mod.rs:908-914`:

```rust
if self.at(TokenKind::Type) {
    self.advance();
    let name = self.expect_type_identifier();
    ...
    return TraitItem::AssocType { name, span };
}
```

`ImplItem::AssocType { name: String, type_expr: TypeExpr, span: Span }`
at `parser/ast.rs:665-670` is parsed at `parser/mod.rs:1136-1149` and
requires `type Name = Type`.

The HIR equivalents (`HirTraitItem::AssocType` at `hir/nodes.rs:483-486`;
`HirImplItem::AssocType` at `hir/nodes.rs:511-515`) are built in
`resolve/mod.rs:940-945` (trait side) and `721-728`, `1024-1033` (impl
side).

`TraitInfo::assoc_types: Vec<String>`
(`resolve/symbols.rs:65`) records the names. `resolve/mod.rs:444`:

```rust
ast::TraitItem::AssocType { name, .. } => assoc.push(name.clone()),
```

### 2.2 Everything downstream of resolve ignores associated types

- `TraitResolver` (`typeck/traits.rs`) has no notion of associated
  types. `check_satisfaction` (lines 85-133) only verifies required
  methods. A trait with an unset associated type on an impl will
  silently check as "satisfied" today if the methods match.
- `Ty` has **no projection variant**. There is no `Ty::Projection { of,
  trait_name, assoc_name }`. Anywhere in a trait signature where
  `Self.Item` appears, resolve today has to pick something to store;
  it falls back to `Ty::TypeParam { name: "Self.Item", bounds: vec![] }`
  (via `resolve/mod.rs:2596-2615` taking the `DefKind::Trait` path,
  then `TypeParam`). This is *wrong* in two ways: it loses the link to
  `Self`, and it treats the projection as a fresh type parameter of
  the enclosing signature.
- The MIR lowerer treats `HirImplItem::AssocType` as a no-op
  (`mir/lower.rs:107`). The associated type is never used in codegen
  because it never reaches MIR.
- No use-site syntax parses today. `Self.Item` in a trait body, or
  `T::Item` / `T.Item` in a function signature, exercises the general
  type-path parser (`parser/types.rs:264-314`) which returns
  `TypePath { segments: ["Self", "Item"] }` — and resolve then fails to
  look it up.

### 2.3 Tutorial already uses associated types

- `docs/tutorial/08-traits.md:27-33`: `trait Iterator` with `type Item`,
  return type `Option[Self.Item]`.
- `docs/tutorial/12-generics.md:105-107`: `where A: Iterable[Item = Int]`
  — the equality-constraint sugar on a trait bound. **This form is the
  surface syntax the parser must accept for associated-type bounds.**

### 2.4 Monomorphization is absent

See overview doc §"Dependency on int64-slot erasure." An associated type
projection that resolves to a concrete field type (`type Item = String`;
then a local `let x: Self.Item`) cannot compile today because every
generic `Ty` becomes `I64` at Cranelift
(`codegen/cranelift.rs:965-984`). Phase 2 of this doc requires the
monomorphization pass to exist (M2 in the overview). Phase 1 is
pure type-check elaboration.

## 3. Goals & Non-goals

### Goals

- **G1.** `trait Iterator; type Item; def mut next -> Option[Self.Item] end`
  parses, resolves, type-checks, and is usable in impl blocks.
- **G2.** `impl Iterator for Vec[Int]; type Item = Int; ...` binds
  `Item` and type-checks every use of `Self.Item` inside the impl body
  against `Int`.
- **G3.** At a use site `fn iter_sum[I: Iterator](i: &mut I) -> I::Item`
  (pick one syntax — see §4), the return type projects to the concrete
  associated type for each monomorphization.
- **G4.** Equality constraints in bounds: `where I: Iterator[Item = Int]`
  compiles and restricts `I` to iterators over `Int`.
- **G5.** An impl that does not bind every required associated type
  produces a clear error (E-ASSOC-MISSING).
- **G6.** Auto-projection: `I::Item` in a generic signature participates
  in type inference (unifies with `Int` when `I` is constrained to
  `Iterator[Item = Int]`).
- **G7.** Super-trait associated types are visible in sub-trait bounds:
  `trait ExactSizeIterator: Iterator` may refer to `Self.Item`.

### Non-goals

- **NG1.** GATs. These are doc 05. The surface syntax here is
  `type Name` (no generic parameters on the associated type).
- **NG2.** `impl Trait` return-position *desugaring* to an associated
  type. Rust does this internally for `async fn` / RPITIT; Riven ships
  this in doc 04.
- **NG3.** Associated consts. `type Item` only — no `const N: Int` in
  traits. Can be added later (see overview §Open #3).
- **NG4.** Defaulted associated types: `type Item = Int`. Feasible
  (Rust has it) but not required for the Iterator use case. Defer.
- **NG5.** Normalization of higher-ranked projections. Requires both
  HRTBs (doc 03) and GATs (doc 05). Out of scope.

## 4. Surface Syntax

### 4.1 Declaration

```riven
trait Iterator
  type Item
  def mut next -> Option[Self.Item]
end

trait FromIterator[T]
  def self.from_iter[I: Iterator[Item = T]](iter: I) -> Self
end

trait IntoIterator
  type Item
  type IntoIter: Iterator[Item = Self.Item]
  def consume into_iter -> Self.IntoIter
end
```

Rules:

- Associated types have no bounds at the `trait` declaration today,
  but may acquire bounds via `type IntoIter: Iterator[Item = Self.Item]`
  syntax (see `IntoIterator` above). The bound is checked at every impl.
- Each associated type is introduced in a `type Name` line, terminated
  by newline. No `end`.
- Associated types live in the trait's namespace. Outside the trait
  body, refer to them as `<Self.Item>` in the trait body (while `Self`
  is in scope) or `<T.Item>` / `<T::Item>` at a use site (syntax in 4.3).

### 4.2 Impl binding

```riven
impl Iterator for Vec[Int]
  type Item = Int
  def mut next -> Option[Int]
    self.pop
  end
end

impl[T] IntoIterator for Vec[T]
  type Item = T
  type IntoIter = VecIntoIter[T]
  def consume into_iter -> VecIntoIter[T]
    VecIntoIter.new(self)
  end
end
```

Rules:

- Every associated type declared on the trait must be bound in the
  impl, else E-ASSOC-MISSING.
- A bound like `type IntoIter: Iterator[Item = Self.Item]` must be
  satisfied by the binding. Riven checks this at impl-registration
  time (§5.3).
- Defaulted associated types (NG4) would skip the required-to-bind
  rule; not in scope.

### 4.3 Use-site projection syntax

Two forms, equivalent:

```riven
# Dot form (Ruby-style, matches tutorial 08:31):
def sum[I: Iterator[Item = Int]](i: &mut I) -> Int
  let mut total = 0
  while let Some(x) = i.next
    total = total + x
  end
  total
end

# Generic-over-Item form:
def sum_all[I: Iterator](i: &mut I) -> I.Item
  where I.Item: Add[Output = I.Item] + Default
  let mut total = I.Item.default
  while let Some(x) = i.next
    total = total + x
  end
  total
end
```

Rules:

- `T.Name` (dot form) is the projection operator. The parser already
  accepts `TypePath { segments: ["T", "Name"] }` (`parser/types.rs:289-299`).
- Resolution rewrites `<T as Trait>.Name` into `Ty::Projection { base:
  T, trait_name: Trait, assoc_name: Name }` (§5.1).
- If `T` is constrained by exactly one trait that declares an
  associated type of that name, the `as Trait` is elided; otherwise
  the user must disambiguate.
- **Disambiguation syntax (decision needed — see §9):** either
  `T.(Iterator.Item)` or `<T as Iterator>.Item`. Recommendation:
  `<T as Iterator>.Item` — matches Rust, lexically unambiguous, and
  uses only existing tokens (`<`, `as`, `>`, `.`).

### 4.4 Equality constraints

```riven
def process[I: Iterator[Item = Int]](i: &mut I) ...
def collect_strings[I: Iterator[Item = String]](i: I) -> Vec[String] ...
```

Sugar for: `I: Iterator where I.Item = Int`.

## 5. Type-System Changes

### 5.1 New `Ty` variant

Add to `hir/types.rs:39-166`:

```rust
/// `<T as Iterator>.Item` — an associated-type projection.
///
/// This is a *delayed* type; it becomes the concrete type once
/// `base` is known and `trait_name` is shown to be implemented.
/// Before normalization it behaves like an opaque type parameter;
/// after normalization it is replaced by the impl's binding.
Projection {
    base: Box<Ty>,
    trait_name: String,
    assoc_name: String,
},
```

Placement: after `TypeParam` (line 139) so that the solver's
"unresolved type variable" path is textually adjacent.

Display form: `<T as Iterator>.Item`. Short form when unambiguous
(inferred from context): `T.Item`.

### 5.2 Resolve rewrites projections

`resolve_type_expr` (`resolve/mod.rs:2300+`): for a `TypePath` whose
last segment is the name of an associated type of exactly one of the
preceding segment's bounds, rewrite into `Ty::Projection`.

Pseudocode (new helper, next to `resolve_type_path` at `resolve/mod.rs:
2527`):

```rust
fn try_project(&self, base_ty: &Ty, assoc_name: &str) -> Option<Ty> {
    // base_ty must be a TypeParam or Self — look at its bounds.
    let bounds = match base_ty {
        Ty::TypeParam { bounds, .. } => bounds,
        _ => return None,
    };
    let candidates: Vec<&TraitRef> = bounds.iter().filter(|tr| {
        self.find_trait_info(&tr.name)
            .map(|t| t.assoc_types.contains(&assoc_name.to_string()))
            .unwrap_or(false)
    }).collect();
    match candidates.len() {
        0 => None, // not an associated type — fall through to normal lookup
        1 => Some(Ty::Projection {
            base: Box::new(base_ty.clone()),
            trait_name: candidates[0].name.clone(),
            assoc_name: assoc_name.to_string(),
        }),
        _ => {
            self.error("E-ASSOC-AMBIG: multiple traits declare this associated type");
            Some(Ty::Error)
        }
    }
}
```

### 5.3 `TraitInfo` grows assoc-type metadata

`resolve/symbols.rs:60-66`:

```rust
pub struct TraitInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub super_traits: Vec<TraitRef>,
    pub required_methods: Vec<String>,
    pub default_methods: Vec<String>,
    pub assoc_types: Vec<AssocTypeDecl>,  // was Vec<String>
}

pub struct AssocTypeDecl {
    pub name: String,
    pub bounds: Vec<TraitRef>,   // e.g., type IntoIter: Iterator[Item = Self.Item]
    pub span: Span,
}
```

The existing `Vec<String>` becomes richer to accommodate the bound
required for `IntoIterator::IntoIter`. Migration: every existing call
site that reads `.assoc_types.contains(&name)` becomes
`.assoc_types.iter().any(|a| a.name == name)`.

### 5.4 Impl-side binding table

New `ImplInfo` stored per-impl (replacing the existing per-type
`nominal_impls: HashMap<(String, String), Vec<ImplMethod>>` in
`TraitResolver`, `typeck/traits.rs:32-36`):

```rust
pub struct ImplInfo {
    pub trait_ref: TraitRef,       // Trait[generic_args]
    pub target_ty: Ty,             // the type we're impl-ing for
    pub methods: Vec<ImplMethod>,
    pub assoc_bindings: HashMap<String, Ty>,  // "Item" -> Int
}
```

Built in `collect_item_impls` (`typeck/traits.rs:189-230`): when
walking an impl, extract `HirImplItem::AssocType { name, ty, .. }`
entries into `assoc_bindings`.

### 5.5 Normalization (projection → concrete)

A new module `typeck/project.rs` provides:

```rust
pub fn normalize(ty: &Ty, resolver: &TraitResolver, symbols: &SymbolTable) -> Ty;
```

Rules:

- For `Ty::Projection { base, trait_name, assoc_name }`:
  - Resolve `base` (may itself contain projections, recurse).
  - Find an impl where `trait_ref.name == trait_name` and
    `target_ty` unifies with `base`.
  - Look up `assoc_bindings[assoc_name]`, substitute the impl's
    generic args, recurse.
  - If `base` is a generic parameter with a matching bound, keep the
    projection (cannot normalize further until monomorphized).
- For all other `Ty`, recurse structurally.

Normalization is called:

- Before every type equality check in `typeck/unify.rs`.
- Before every subtype / coerce check in `typeck/coerce.rs`.
- At monomorphization time (M2), when all generics have concrete
  substitutions.

### 5.6 Equality-constraint sugar

Parser change: in `parse_trait_bounds` / `parse_single_trait_bound`
(`parser/types.rs:390-405`), accept `Name[AssocName = Type]` where
`AssocName` is an uppercase identifier and `Type` is a type expression.
Build an AST `AssocEqConstraint { name, ty }` and attach to the bound.

AST addition (`parser/ast.rs` near `TraitBound:100-104`):

```rust
pub struct TraitBound {
    pub path: TypePath,
    pub assoc_eq: Vec<AssocEqConstraint>,
    pub span: Span,
}

pub struct AssocEqConstraint {
    pub name: String,
    pub ty: TypeExpr,
    pub span: Span,
}
```

Resolve walks these into `TraitRef` (extended with an
`assoc_eq: Vec<(String, Ty)>` field).

During solving, `T: Iterator[Item = Int]` is the conjunction of
`T: Iterator` and `<T as Iterator>.Item = Int`. The projection
normalizer sees the `= Int` side and records the binding in the local
substitution map.

### 5.7 Impl consistency check

New pass `typeck/check_impls.rs`, runs after `TraitResolver::collect_impls`:

- For every impl, verify every associated type declared on the trait
  is bound in the impl (else E-ASSOC-MISSING at impl.span).
- For every bound on an associated-type declaration (§5.3 bounds),
  verify the impl's binding satisfies it via normal trait solving.
  E.g., `type IntoIter: Iterator[Item = Self.Item]` + `type IntoIter = VecIntoIter[T]`
  + `type Item = T` → check `VecIntoIter[T]: Iterator[Item = T]`.
- Reject duplicate bindings (E-ASSOC-DUP).

## 6. Implementation Plan

### 6.1 Code map

| Change | File(s) |
|---|---|
| Add `Ty::Projection` | `hir/types.rs:39-166` |
| Display/Debug for `Ty::Projection` | `hir/types.rs:344-478` |
| AST `AssocEqConstraint` + `TraitBound.assoc_eq` | `parser/ast.rs:98-104` |
| Parse `Trait[Assoc = Type]` sugar | `parser/types.rs:400-405` |
| `TraitInfo.assoc_types: Vec<AssocTypeDecl>` | `resolve/symbols.rs:60-66` |
| `TraitRef.assoc_eq: Vec<(String, Ty)>` | `hir/types.rs:14-18` |
| Resolve projection `T.Name` → `Ty::Projection` | new helper in `resolve/mod.rs` near `resolve_type_path` |
| `ImplInfo.assoc_bindings` | `typeck/traits.rs:32-48` (rewrite) |
| Normalizer | new `typeck/project.rs` |
| Call normalizer in unify/coerce | `typeck/unify.rs`, `typeck/coerce.rs` |
| Impl-completeness pass | new `typeck/check_impls.rs` |
| Error variants | `diagnostics/` |
| MIR: projections erased by monomorphization | `mir/lower.rs` post-M2 |
| Fixture tests | `crates/riven-core/tests/fixtures/assoc_*.rvn` |

### 6.2 Phasing

**Phase 01a — type-check front-end (3 weeks, no codegen change).**

1. Extend `TraitInfo` and `TraitRef` with the new fields.
2. Parser: `[Assoc = Type]` sugar.
3. Resolve: rewrite `T.Name` to `Ty::Projection` when `T` is a
   constrained type parameter.
4. Normalizer for projections over concrete types (no generics).
5. `check_impls.rs`: completeness and consistency.
6. Error messages.

At the end of phase 01a, `trait Iterator` parses; impls bind
`type Item`; function signatures mentioning `I.Item` type-check; but
generic functions *calling* `i.next` still use the erased
`I64` slot at codegen. `impl Iterator for Vec[Int]` works as a
stand-alone test if the function is called on a concrete `Vec[Int]`
directly (the projection normalizes to `Int` at the call site).

**Phase 01b — generic monomorphization hook (blocks on M2).**

7. Post-monomorphization, every `Ty::Projection` has a concrete base
   and normalizes to a concrete type. MIR lowering sees only concrete
   types.
8. Remove the Cranelift/LLVM `I64` fallback for generic fields that
   project to known types.
9. Drop-glue per-instantiation: if `type Item = String`, the Vec's
   drop pass (per tier-1 doc 04 §7) sees `Ty::String` per
   instantiation and emits `riven_string_free` per element.

**Phase 01c — stdlib Iterator.** Depends on 01a + 01b + doc 01
phase 1a of tier-1.

10. Declare `trait Iterator { type Item; def mut next -> Option[Self.Item]; end }`
    as a *user-level* trait in the prelude, not a built-in with
    special-case resolution (the current list at `resolve/mod.rs:
    139-151` just carries a name; this is adequate).
11. Write `impl Iterator for VecIter[T]`, `VecIntoIter[T]`, `SplitIter`,
    `Range`, `RangeInclusive` — each with a real `next` that is not
    `riven_noop_passthrough`. Tier-1 doc 01 §6 lists the concrete
    type list; each gains an `impl Iterator` block.
12. Add the default methods `map`, `filter`, `take`, `skip`, `enumerate`,
    `chain`, `collect` to the trait body. These are legitimate default
    methods — they call `self.next` and reuse the Iterator machinery.

## 7. Interactions With Other Tier-2 Features

### 7.1 With GATs (doc 05)

GATs are associated types that *themselves* take generic parameters.
The design here is deliberately aligned with GATs: `AssocTypeDecl` and
`assoc_bindings` each become richer (the name maps to a "type-level
function" of N arguments rather than a concrete type). Doc 05 §3
extends `AssocTypeDecl` with `generic_params: Vec<GenericParamInfo>`;
the rest of the pipeline is unchanged except the normalizer now
applies those generics at projection sites.

**Do not merge the two specs.** Shipping GATs first is possible but
has no consumers — every current use case (Iterator, IntoIterator,
FromIterator, Future) is a plain associated type.

### 7.2 With trait objects (doc 06)

A `dyn Trait` over a trait with associated types requires the trait
either to:

- forbid being used as `dyn` unless every associated type is *bound*
  at the use site: `dyn Iterator[Item = Int]` — this is the common
  case and the one Rust picked, and
- include the associated types in the vtable's type-info slot
  (expensive and unused today).

Recommendation: `dyn Trait` is only object-safe if every associated
type has a binding at the use site, and that binding is recorded in
the resulting `Ty::DynTrait`. Doc 06 §4 item 5 codifies this.

### 7.3 With variance (doc 07)

An associated type projection is *invariant* with respect to the base
unless declared otherwise. `<I as Iterator>.Item` cannot be subtyped
to anything except the exact binding, because the same `Iterator` impl
is indexed on the concrete type of `I`. Doc 07 §5 enumerates this
rule and explains why it differs from tuple/container variance.

### 7.4 With HRTBs (doc 03)

Normalization interacts poorly with HRTBs when the projection mentions
a bound lifetime: `for['a] I: Iterator[Item = &'a T]`. The "for all 'a"
quantifier moves the projection into a higher-ranked context; doc 03
§7 describes the conservative approach (delay normalization under
quantifiers) that covers our use cases.

### 7.5 With specialization (doc 04 §9)

Multiple impls with overlapping trait bounds + different associated
type bindings is the classic specialization case. If specialization is
omitted (overview rec #5), the coherence check forbids it, and
`impl Iterator for Vec[T]` must have exactly one definition of
`type Item`. If specialization is shipped, the most-specific impl
wins, and the type-checker must remember which associated-type
binding to use at each call site.

## 8. Phasing

See §6.2. Summary:

- **01a (3 weeks, front-end):** parsing, resolve, normalization for
  concrete bases, impl completeness. Ships as a no-op runtime feature
  ( every associated-type-using function still erases to `I64`
  because monomorphization hasn't landed).
- **01b (blocks on M2):** generic codegen via monomorphization.
- **01c (blocks on tier-1 doc 01 phase 1a):** stdlib Iterator.

## 9. Open Questions & Risks

- **OQ-1: projection syntax.** `T.Item` vs `T::Item` vs both. See
  overview §Open #2. Recommendation: `T.Item`. Accept both in the
  parser; formatter rewrites `::` to `.`. Ambiguity with field
  access is resolved by capitalization (`Item` vs `item`).
- **OQ-2: disambiguation syntax.** When `T: Iterator + Other` and
  both declare `Item`, which syntax do we write? Rust uses
  `<T as Iterator>::Item`. Recommendation: copy Rust — this is a
  corner case, a verbose syntax is fine.
- **OQ-3: early vs late normalization.** Rust has a reputation for
  subtle bugs around "I didn't normalize early enough, so inference
  missed a constraint." Riven's type-checker is simpler (no
  higher-ranked solve in 01a). Recommendation: normalize eagerly in
  01a, and revisit when HRTBs land.
- **OQ-4: defaulted associated types.** Out of scope for tier 2; will
  users miss it? Probably not for Iterator; check with stdlib consumers
  once the trait lands.
- **OQ-5: what about `type Iter: Iterator<Item = Self.Item>` where
  `Item` has not been declared yet?** Enforce declaration order:
  associated types must be declared before they are referenced. This
  matches Rust and avoids circular normalization.
- **R-1:** the `Ty::Projection` → `Ty::Infer` fallback in the resolver
  today (via `DefKind::Trait` → `TypeParam`, `resolve/mod.rs:2609-2615`)
  must be removed. Any code that currently type-checks only because of
  this fallback will break. Expect test-fixture churn.
- **R-2:** normalization under Drop check. A `Vec[I.Item]` field
  where `I.Item = String` needs drop; a field where `I.Item = Int`
  does not. The drop-elaboration pass (tier-1 doc 04 §7) consumes
  the post-monomorphization types, so this just works — but only
  because M2 lands first. Sequencing matters.
- **R-3:** the existing `DefKind::Trait { info }` path in
  `resolve_type_path` (`resolve/mod.rs:2609`) turns a bare trait name
  used as a type into a `Ty::TypeParam`. With projections, bare
  `Iterator` as a type becomes `impl Iterator` or `dyn Iterator`; the
  resolver must no longer silently invent a type parameter. This is
  a blocking pre-cleanup for 01a.

## 10. Test Matrix

### 10.1 Positive tests

- T1: declare `trait Iterator` with `type Item`; impl for `Vec[Int]`
  binds `type Item = Int`; call `i.next` on `Vec[Int]::iter`; return
  type is `Option[Int]`.
- T2: generic function `def sum[I: Iterator[Item = Int]](...)` —
  monomorphizes, type-checks, runs.
- T3: projection at use site: `fn first[I: Iterator](i: &mut I) -> Option[I.Item]`.
  Unification of the return at the call site with a concrete iterator
  yields the concrete item type.
- T4: super-trait referring to sub-trait's `Self.Item`:
  `trait DoubleEndedIterator: Iterator`; impl verifies `Self.Item`
  matches the parent binding.
- T5: equality-constraint sugar: `where I: Iterator[Item = String]`
  accepts iterators over `String`, rejects iterators over `&str`.
- T6: `IntoIterator` with a bound on `IntoIter`: `type IntoIter:
  Iterator[Item = Self.Item]` — verify the impl's binding satisfies
  the bound.

### 10.2 Negative tests

- N1: impl missing `type Item` → E-ASSOC-MISSING.
- N2: impl binding `type Item = String` but method signature says
  `Option[Int]` → E-ASSOC-METHOD-MISMATCH (reuses existing type error
  wording).
- N3: `I.Item` where `I` is unconstrained → E-ASSOC-UNBOUND.
- N4: two traits declare `Item`, projection is ambiguous → E-ASSOC-AMBIG
  (suggest `<T as Iterator>.Item`).
- N5: equality-constraint sugar references a name that is not an
  associated type of the bound → E-ASSOC-UNKNOWN.
- N6: projection used as a type constructor (`I.Item[U]`) where the
  associated type is not generic → E-ASSOC-NOT-GENERIC.
- N7: circular: `type Iter: Iterator[Item = Self.Iter]` →
  E-ASSOC-CYCLE at impl time.

### 10.3 Fixture additions

- `tests/fixtures/assoc_basic.rvn` — Iterator on Vec[Int].
- `tests/fixtures/assoc_generic.rvn` — generic function over
  `I: Iterator[Item = T]`.
- `tests/fixtures/assoc_bound_chain.rvn` — IntoIterator with
  `type IntoIter: Iterator[Item = Self.Item]`.
- `tests/fixtures/assoc_error_missing.rvn` — negative: impl missing
  associated type.
