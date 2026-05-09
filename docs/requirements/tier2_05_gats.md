# Tier 2.05 — Generic Associated Types (GATs)

Status: Draft (requirements)
Owner: compiler
Depends on: associated types (doc 01); HRTBs (doc 03, phase 03b) for
            the quantified-projection cases; monomorphization (M2)
Blocks: `LendingIterator`, `StreamingIterator`, `Pool` /
        `Collection` traits that return borrowed views, any trait with
        an associated type that itself takes a lifetime

## 1. Summary & Motivation

A *generic associated type* is an associated type that takes its own
generic parameters — typically lifetimes, occasionally types. The
canonical example:

```riven
trait LendingIterator
  type Item['a]
  def mut next['a](self: &'a mut Self) -> Option[Self.Item['a]]
end
```

`Item['a]` is an associated type that varies with the lifetime `'a`.
An impl for `WindowIter` over a slice:

```riven
impl LendingIterator for WindowIter[Int]
  type Item['a] = &'a [Int]
  def mut next['a](self: &'a mut Self) -> Option[&'a [Int]] ...
end
```

The caller of `next` gets a borrow tied to the lifetime of the
receiver — expressible only if `Item` can depend on `'a`. Without
GATs, the caller would be forced to `clone` every item (turning
`LendingIterator` into a plain `Iterator[Item = Vec[Int]]`) or the
impl would have to promote items to owned types.

**Why Riven needs GATs:**

1. **Pools of borrowed resources.** `struct Pool[T]; pool.acquire -> impl Handle['_ Pool]`
   where the handle's lifetime is tied to the pool.
2. **Streaming parsers.** A parser that yields borrowed slices from
   an input buffer — cannot be expressed as `Iterator[Item = &Str]`
   with a fixed lifetime.
3. **Cursor-like APIs.** `Vec.chunks_mut[N]` returns a lending
   iterator of `&mut [T; N]`.
4. **Async iteration.** `async fn next` on a stream is an associated
   type generic over the output future (blocks tier-1 async doc 03).

GATs took six years to stabilize in Rust (RFC 1598 in 2016; stable in
late 2022). Most of the delay was not "how do I lower this" but
"how do I make the solver halt and produce understandable error
messages." This doc addresses the lowering and surface syntax;
solver correctness is called out as the primary risk (§9).

## 2. Current State

### 2.1 Associated types exist in name only

See doc 01 §2. `HirTraitItem::AssocType { name, span }`
(`hir/nodes.rs:483-486`) has no generic parameters. Adding them is
this doc's main structural change.

### 2.2 Nothing else exists

No GAT-specific code. The doc-01 infrastructure (`Ty::Projection`,
`ImplInfo.assoc_bindings`, normalization) is the foundation; GATs
extend every part with a generic-args slot.

## 3. Goals & Non-goals

### Goals

- **G1.** `trait T; type A['a]; type B[U, 'a]` — associated types
  with lifetime and type parameters.
- **G2.** `impl T for S; type A['a] = &'a Str; type B[U, 'a] = Map['a, U]`
  — impls bind the generic associated type.
- **G3.** Use-site projection with generic args: `<Self as T>.A['b]`,
  `I.A['x]`, `I.B[Int, 'x]`.
- **G4.** `LendingIterator::next` with lifetime-dependent return:
  type-checks, borrow-checks.
- **G5.** Where-clause constraints on GATs:
  `where for['a] I.Item['a]: Display`.

### Non-goals

- **NG1.** GATs on traits that are also `dyn`. See §7.3.
- **NG2.** Higher-kinded unification — GATs in unification sites
  outside projections (`F: for['a] Fn(&'a T) -> T::Item['a]`) are
  accepted when the expression is syntactically a projection, rejected
  otherwise.
- **NG3.** Defaulted GATs. Rust has them; they are rarely needed.
  Defer.
- **NG4.** Recursive GATs (a GAT that mentions itself). Solver
  hazard; forbid at declaration.

## 4. Surface Syntax

### 4.1 Declaration

```riven
trait LendingIterator
  type Item['a]
  def mut next['a](self: &'a mut Self) -> Option[Self.Item['a]]
end

trait Collection
  type Iter['a]: Iterator[Item = &'a Self.Elem]
  type Elem
  def iter['a](self: &'a Self) -> Self.Iter['a]
end
```

Rules:

- Parameters in `[...]` immediately after the associated type's name.
- Parameters are introduced the same way as on a function: name,
  optional colon-bound, lifetime syntax.
- Bounds on the associated type itself (`type Iter['a]: Iterator[...]`)
  may reference the associated type's own parameters.
- Declaration order: GATs are declared before methods that reference
  them. (Solver hazard mitigation; see §9.)

### 4.2 Impl binding

```riven
class WindowIter[T]
  data: Vec[T]
  pos: USize
  width: USize
  def init(@data: Vec[T], @width: USize)
    self.pos = 0
  end
end

impl[T] LendingIterator for WindowIter[T]
  type Item['a] = &'a [T]
  def mut next['a](self: &'a mut Self) -> Option[&'a [T]]
    if self.pos + self.width > self.data.len
      return None
    end
    let slice = &self.data[self.pos..self.pos + self.width]
    self.pos = self.pos + 1
    Some(slice)
  end
end
```

Rules:

- The impl's `type Item['a] = ...` introduces `'a` as a binder; the
  RHS may use `'a`. The name on the RHS must match the name on the
  declaration (`'a`), or both sides must be alpha-renamed consistently.
- The impl's method signature's `'a` is the *same* lifetime as the
  associated-type's `'a` — they are unified.

### 4.3 Use-site projection

```riven
def first_window['a, T](w: &'a mut WindowIter[T]) -> Option[&'a [T]]
  w.next()
end

def print_windows[W: LendingIterator](w: &mut W)
  where for['a] W.Item['a]: Displayable
  while let Some(item) = w.next
    puts item.to_display
  end
end
```

Rules:

- `Self.Item['a]` inside a trait body: projection with the trait's
  generic arg passed through.
- `W.Item['a]` outside: the `['a]` is a new lifetime, quantified by
  the surrounding `for['a]` if present, else bound to a specific
  lifetime.
- Where-clauses accept HRTB on GAT projections:
  `where for['a] W.Item['a]: Display`.

## 5. Type-System Changes

### 5.1 `AssocTypeDecl` gains generic params

From doc 01 §5.3:

```rust
pub struct AssocTypeDecl {
    pub name: String,
    pub generic_params: Vec<GenericParamInfo>,  // NEW: ['a], ['a, U], etc.
    pub bounds: Vec<TraitRef>,
    pub span: Span,
}
```

### 5.2 `Ty::Projection` gains generic args

From doc 01 §5.1:

```rust
Projection {
    base: Box<Ty>,
    trait_name: String,
    assoc_name: String,
    generic_args: Vec<Ty>,   // NEW
},
```

Empty for plain associated types (doc 01); populated for GATs.

### 5.3 `ImplInfo.assoc_bindings` stores type-level functions

From doc 01 §5.4:

```rust
pub struct AssocBinding {
    pub generic_params: Vec<GenericParamInfo>,  // NEW
    pub ty: Ty,                                  // may mention generic_params
}

pub struct ImplInfo {
    pub trait_ref: TraitRef,
    pub target_ty: Ty,
    pub methods: Vec<ImplMethod>,
    pub assoc_bindings: HashMap<String, AssocBinding>,
}
```

A `type Item['a] = &'a Str` binding is stored as:

```
AssocBinding {
    generic_params: [Lifetime("a")],
    ty: Ty::RefLifetime("a", Ty::Str),
}
```

### 5.4 Normalization with generic args

`normalize` from doc 01 §5.5, extended:

- For `Ty::Projection { base, trait_name, assoc_name, generic_args }`:
  - Resolve `base`.
  - Find the impl.
  - Look up `assoc_bindings[assoc_name]`.
  - Substitute the impl's generic args into the binding's `ty`.
  - *Substitute the projection's `generic_args` into the binding's
    `generic_params`.*
  - Normalize recursively.

### 5.5 Borrow-check interaction

The associated type's lifetime parameter unifies with the function-
parameter lifetime at the call site. Example:

```
w.next                              # W is a &mut WindowIter[Int]
  ^^^^
  // self.next['l0](self: &'l0 mut W)
  // returns Option[W.Item['l0]]
  // which normalizes via the impl to
  // Option[&'l0 [Int]]
```

The skolem `'l0` is the lifetime of the receiver borrow. Rust's
"soundness of GATs" work proved that as long as the associated-type
bindings are treated as ordinary substitutions and the borrow
checker's region relation is transitive, the result is sound.

### 5.6 No codegen change (beyond doc 01)

GATs monomorphize identically to associated types: at each
instantiation, every projection normalizes to a concrete type. The
compiled body does not "know" the GAT exists.

## 6. Implementation Plan

### 6.1 Code map (deltas over doc 01)

| Change | File(s) |
|---|---|
| AST `TraitItem::AssocType.generic_params` | `parser/ast.rs:644-648` |
| Parse `type Name['a, T]` | `parser/mod.rs:905-914` |
| AST `ImplItem::AssocType.generic_params` | `parser/ast.rs:665-670` |
| Parse `type Name['a] = Type` | `parser/mod.rs:1136-1149` |
| `AssocTypeDecl.generic_params` | `resolve/symbols.rs:60-66` |
| `AssocBinding.generic_params` | `typeck/traits.rs` |
| `Ty::Projection.generic_args` | `hir/types.rs` |
| Normalizer substitutes both sides | `typeck/project.rs` |
| HRTB + GAT: `for['a] W.Item['a]: Display` | `typeck/solver.rs` |
| Error codes | `diagnostics/` |

### 6.2 Phasing

**Phase 05a — declarations and bindings (2 weeks, depends on 01a).**

1. Parser: accept `[...]` on trait and impl `type` items.
2. Resolver: walk into AssocTypeDecl / AssocBinding with generics.
3. HIR: extend `Ty::Projection`.
4. Normalization: substitute both the base impl's generics and the
   projection's generics.

**Phase 05b — use-site solving (2 weeks, depends on 03b — HRTBs).**

5. Where-clauses `for['a] T.Item['a]: Bound`.
6. Solver handles quantified GAT projections with skolems.
7. Error messages for GAT solve failures (the famous class of
   "type may not live long enough" errors in Rust).

**Phase 05c — stdlib additions (1-2 weeks, depends on 05a).**

8. `LendingIterator` trait in the prelude.
9. `Vec.windows[N]`, `Vec.chunks_mut` adapters.
10. Documentation and tutorial chapter.

## 7. Interactions With Other Tier-2 Features

### 7.1 With associated types (doc 01)

GATs generalise associated types. Doc 01 phase 01a is a hard
prerequisite; GATs reuse every data structure, extended with one
extra slot for generic params.

### 7.2 With HRTBs (doc 03)

HRTBs are required for the typical GAT bound:
`where for['a] I.Item['a]: Display`. Doc 03 phase 03b is a hard
prerequisite for 05b.

### 7.3 With trait objects (doc 06)

GAT-using traits are **not object-safe** unless every GAT is bound
at the use site (like plain associated types, doc 01 §7.2; extended
for GATs because the bound must cover every generic arg of the GAT).

Practically, `dyn LendingIterator` is forbidden because the vtable
would need to include a function from lifetime → type, which is
gibberish. Doc 06 §4 item 5 enforces this.

### 7.4 With variance (doc 07)

GAT parameters have variance of their own. `type Item['a] = &'a T`
is covariant in `'a`. `type Item['a] = &'a mut T` is invariant.
Variance inference (doc 07 §6) treats each GAT parameter as an
extra column in the variance table.

### 7.5 With const generics (doc 02)

`type Buf[const N: USize] = [T; N]` is a legal declaration. Rare
but supported; no new logic beyond already supporting `[T; N]` with
const-generic `N`.

### 7.6 With impl Trait (doc 04)

`fn iter[T] -> impl LendingIterator[Item['a] = &'a T]` — opaque type
that itself mentions a GAT. Hidden type inference must be aware of
GATs. Integrated into doc 04 phase 04b.

### 7.7 With specialization

As in doc 01, GATs interact with specialization to produce
lifetime-soundness issues. Reinforces the "no specialization" recommendation in doc 04 §9.

## 8. Phasing

See §6.2.

## 9. Open Questions & Risks

- **OQ-1: same identifier for parameter on trait side and impl side?**
  `trait T; type A['a]` and `impl T for S; type A['b] = ...` —
  must the impl use `'a`? Rust is lenient, accepts either. Riven
  follows: alpha-rename on parse.
- **OQ-2: bound propagation.** `type Iter['a]: Iterator[Item = &'a
  Self.Elem]` — at impl time, must we check that the binding satisfies
  `Iterator[Item = &'a Self.Elem]` for every possible `'a`? Yes, via
  HRTB introduction.
- **OQ-3: recursive GATs.** `type A['a] = Option[Self.A['a]]` — the
  type mentions itself. Rust forbids with a solver cycle error.
  Riven: detect cycles at normalize time, E-GAT-CYCLE.
- **OQ-4: GAT with a type parameter bound by `Self.Item` of another
  trait.** Solver ordering question — does resolution of one GAT
  happen before another? Document: GATs in a single impl are resolved
  left-to-right as written.
- **OQ-5: projections with unknown generic args.** `T.Item['?]`
  where the lifetime is an inference variable. Handling depends on
  whether the variable is skolem or unification. Treat as "defer
  normalization" until more information arrives.
- **R-1: solver soundness.** This is the Rust-was-burned-for-six-years
  risk. Mitigation: copy Rust's `ImplTraitInTraitDelayedSubstitution`
  approach, run solver fuzz tests, accept that some pathological
  programs will hang until a depth limit triggers E-SOLVER-DEPTH.
- **R-2: error message quality.** GAT errors are the notorious
  "type may not live long enough" class. Invest in a dedicated error
  formatter that points at both the trait declaration and the impl
  binding.
- **R-3: developer ergonomics.** Even Rust users struggle. Document
  prominently; provide worked examples in the tutorial. Consider
  auto-suggesting `where for['a] I.Item['a]: Trait` when a naive
  bound fails.

## 10. Test Matrix

### 10.1 Positive tests

- T1: `trait LendingIterator` + `impl for WindowIter[Int]`.
  `w.next` returns `Option[&[Int]]` with the right lifetime.
- T2: Consuming caller: `while let Some(x) = w.next { puts x.len }`.
- T3: GAT with multiple params: `type Entry['a, K] = (&'a K, V)`.
- T4: GAT with bound: `type Iter['a]: Iterator[Item = &'a Self.Elem]`
  + impl satisfying the bound.
- T5: HRTB on GAT projection: `where for['a] W.Item['a]: Display`.

### 10.2 Negative tests

- N1: impl binds `type Item['a] = Vec[T]` (drops `'a`); bound
  `where for['a] I.Item['a]: Display` is fine. *Positive*. But
  impl's GAT declaration that says `type Item['a]: Display` and an
  impl binding `Vec[T]` where `T` isn't `Display` → E-GAT-BOUND-VIOL.
- N2: Recursive GAT → E-GAT-CYCLE.
- N3: GAT with lifetime mismatch: impl declares `['a]` on the type but
  signature uses `['b]` → E-GAT-LIFETIME-UNBOUND (or alpha-rename
  silently — per OQ-1, alpha-rename).
- N4: `dyn LendingIterator` → E-DYN-NOT-SAFE (GAT makes the trait
  non-object-safe unless bound).
- N5: Solver cycle depth exceeded (pathological trait with
  mutually-recursive GATs) → E-SOLVER-DEPTH.

### 10.3 Fixture additions

- `tests/fixtures/gat_lending.rvn` — LendingIterator over
  `Vec.windows`.
- `tests/fixtures/gat_collection.rvn` — `Collection` with
  `type Iter['a]: Iterator`.
- `tests/fixtures/gat_error_dyn.rvn` — negative: dyn on GAT trait.
- `tests/fixtures/gat_error_cycle.rvn` — negative: recursive GAT.
