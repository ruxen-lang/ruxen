# Tier 2.04 — `impl Trait` and Specialization

Status: Draft (requirements)
Owner: compiler
Depends on: monomorphization (M2); associated types (doc 01) for
            return-position `impl Trait` that desugars to an associated
            type
Blocks: the cleanest signature for iterator combinators; return-position
        `impl Fn`, `impl Iterator` from stdlib

## 1. Summary & Motivation

This doc bundles two features that are historically tangled:

- **`impl Trait`** in argument position and return position — opaque
  existential types that let the caller or callee hide a concrete type
  behind a trait bound.
- **Specialization** — the ability to write multiple overlapping impls
  and let the compiler pick the most specific one at each call site.

They are bundled because (a) return-position `impl Trait` historically
pressured Rust toward specialization for the `impl Display for T` case
(a blanket impl plus a specific override), and (b) both are about
narrowing a generic signature at compile time. But they are *not* the
same feature and can ship independently.

**The recommendation, summarised:**

- **Ship `impl Trait` in argument position in phase 04a** — it is a
  trivial desugaring to a fresh type parameter plus a bound, already
  half-implemented (`Ty::ImplTrait` exists at `hir/types.rs:113`).
- **Ship `impl Trait` in return position in phase 04b**, blocked on
  monomorphization (M2) and a notion of opaque types (§5.2).
- **Do not ship specialization in v1.** See §9. Document the feature
  request, reserve the syntax, and defer to a future release.

## 2. Current State

### 2.1 `Ty::ImplTrait` exists and is parsed

`hir/types.rs:112-113`:

```rust
/// `impl Trait` — static dispatch, structural satisfaction OK
ImplTrait(Vec<TraitRef>),
```

Parser: `parser/types.rs:47-51, 205-211`. `TypeExpr::ImplTrait { bounds,
span }` (`parser/ast.rs:76-79`).

Resolver: `resolve/mod.rs:2484-2491` builds `Ty::ImplTrait(Vec<TraitRef>)`.

### 2.2 Layout treats `ImplTrait` as pointer-sized

`codegen/layout.rs:337-339`:

```rust
// ── impl Trait (static dispatch) ────────────────────────────────────
// The concrete type is erased here; we conservatively return pointer size.
Ty::ImplTrait(_) => TypeLayout::primitive(8, 8),
```

This is wrong for anything larger than a pointer, and gets us back to
the int64-slot-erasure problem (overview §Dependency). `impl Trait`
return position cannot ship until M2 lands.

### 2.3 Trait resolution accepts structural satisfaction

`typeck/traits.rs:83-125`: for `impl Trait`, structural satisfaction
is accepted (the type has methods with matching signatures, no
explicit `impl T for U` required). For `dyn Trait`, nominal only.

This is the *semantic difference* between `impl` and `dyn` that must
be preserved through any refactor. It also means `impl Trait` cannot
be stored — a structurally-satisfied type has no vtable.

### 2.4 No `impl Trait` in return position

Parser accepts the syntax in return types (same `TypeExpr::ImplTrait`),
but the resolver / typeck treats it the same as an argument-position
`impl Trait` — a fresh type parameter. For return position this is
wrong: the concrete type must not leak to the caller. The caller sees
an opaque type.

No fixture uses `impl Trait` in return position today.

### 2.5 Specialization — nothing exists

No `impl` overlap check. `TraitResolver::register_impl`
(`typeck/traits.rs:59-79`) uses `HashMap<(String, String), Vec<ImplMethod>>`
keyed by `(type_name, trait_name)` — there is silently a *single*
entry per pair. Two `impl Foo for Vec[Int]` in the same program would
overwrite each other with no diagnostic.

This is a pre-existing coherence hole (tier 1 does not surface it;
see §9 risk).

## 3. Goals & Non-goals

### Goals

- **G1.** `def foo(x: impl Displayable)` — argument-position `impl
  Trait` is a syntactic sugar for `def foo[T: Displayable](x: T)`. Type-
  check, monomorphize, run.
- **G2.** `def iter() -> impl Iterator[Item = Int]` — return-position
  `impl Trait` produces an opaque type whose concrete body is the
  function's return type, erased from the caller's view.
- **G3.** Two call sites of a return-position `impl Trait` function
  see *the same opaque type* — not "any iterator," but "the specific
  iterator this function returns." Needed for e.g. passing the return
  value back into the same function.
- **G4.** Argument-position `impl Trait` does not monomorphize once
  per call site (the whole point is one body per distinct concrete
  type, same as any generic).
- **G5.** `impl Trait` is not storable in a field unless the field's
  type is the same function's return type via a `type` alias — i.e.,
  `type MyIter = impl Iterator[Item = Int]` (phase 04c).
- **G6.** Coherence: multiple `impl` blocks for the same `(trait,
  type)` pair produce E-IMPL-DUP at impl-register time. Fixes the
  pre-existing hole in §2.5.

### Non-goals

- **NG1.** Specialization. See §9.
- **NG2.** Trait alias (`trait Foo = Iterator + Clone`). Separate
  feature; frequently paired with `impl Trait` but independent.
- **NG3.** `impl Trait` in trait method signatures (RPITIT). Rust
  shipped this in 2023; requires careful interaction with associated
  types and sendability. Defer.
- **NG4.** `impl Trait` in `let` bindings. Rust did not stabilize.
- **NG5.** Named opaque types (`type MyIter: Iterator[Item = Int]`).
  Phase 04c only if requested by stdlib.

## 4. Surface Syntax

### 4.1 Argument-position `impl Trait`

```riven
def print_all(items: impl Iterator[Item = String])
  for s in items
    puts s
  end
end

# Equivalent:
def print_all[I: Iterator[Item = String]](items: I) ...
```

Multiple bounds:

```riven
def log(x: impl Displayable + Serializable)
  puts x.to_display
  save(x.serialize)
end
```

`&impl Trait`, `&mut impl Trait` work the usual way.

### 4.2 Return-position `impl Trait`

```riven
def ones -> impl Iterator[Item = Int]
  (1..).map { |_| 1 }
end

def make_closure(x: Int) -> impl Fn(Int) -> Int
  { |y| x + y }
end
```

Rules:

- The concrete return type is inferred from the body.
- Every `return` in the function must produce the same concrete
  type (E-IMPL-RET-MISMATCH). Different types with a common trait
  are not collapsed; the user must go through `Box[dyn Trait]` (doc
  06) if they want heterogeneity.
- The caller sees an opaque type that implements the declared traits.

### 4.3 Named opaque types (phase 04c, optional)

```riven
type FizzBuzzIter = impl Iterator[Item = String]

def fizzbuzz -> FizzBuzzIter
  (1..101).map { |n| ... }
end
```

Allows storing the opaque type in struct fields.

## 5. Type-System Changes

### 5.1 Argument-position lowering (phase 04a)

In the resolver, `Ty::ImplTrait(bounds)` in an argument position is
replaced by a fresh synthetic generic parameter:

```rust
// In resolve_func_def when walking params:
let param_ty = self.resolve_type_expr(&param.type_expr);
let concrete_ty = match param_ty {
    Ty::ImplTrait(bounds) => {
        let synth_name = format!("__impl_{}", self.fresh_impl_id());
        let gp = HirGenericParam {
            name: synth_name.clone(),
            bounds,
            span: param.span.clone(),
        };
        self.add_generic_param_to_enclosing(gp);
        Ty::TypeParam { name: synth_name, bounds }
    }
    other => other,
};
```

The synthetic generic parameter is indistinguishable from a
user-written one after this pass. Monomorphization (M2) treats it
normally.

### 5.2 Return-position opaque type (phase 04b)

Return position is fundamentally different. The type must be hidden
from the caller:

```rust
pub enum Ty {
    ...
    /// An opaque type defined by a specific function.
    /// `def_id` identifies the defining function; `bounds` are the
    /// declared traits; the concrete body is inferred post-body-check.
    Opaque {
        def_id: DefId,
        bounds: Vec<TraitRef>,
    },
    ...
}
```

Resolver:

- For each `impl Trait` *in return position*, synthesise a
  `Ty::Opaque { def_id: enclosing_fn, bounds }`.
- Attach a "hidden-type" slot to the function's `FnSignature`:
  `pub hidden_return_ty: Option<Ty>` — filled in by typeck once the
  body is analysed.

Typeck:

- When checking the body, infer the concrete return type normally.
- Store it in `hidden_return_ty`.
- Check that the inferred type satisfies every declared bound.

Consumer of an opaque type:

- Caller sees `Ty::Opaque`. Method calls dispatch through the
  declared bounds (same as `ImplTrait` in argument position).
- The *compiler* (not user code) can "peek through" the opaque type
  at monomorphization, inlining the concrete body. The concrete type
  is used for layout, drop-elaboration, and to decide whether the
  opaque can be passed to another function with the same opaque
  signature.

### 5.3 Opaque-type identity

Two calls to `ones` above return values of the *same* opaque type —
the one defined by `ones`. Storing one in a `let` and the other in
another `let`, then comparing with `==`, is only meaningful if the
iterator type itself defines `PartialEq`, which is rare. But
*assigning one to the other* is well-typed: `let a: typeof(ones) = ones()`;
`let b: typeof(ones) = ones(); let c = a; c = b` should type-check
because `a` and `b` have the same `Ty::Opaque { def_id: ones, .. }`.

### 5.4 Coherence check

New pass `typeck/coherence.rs` runs after all impls are collected:

- For every `(trait_name, target_ty_skeleton)` pair, count impls.
  > 1 → E-IMPL-DUP.
- "Skeleton" means: compare structurally, treating generic params as
  alpha-equivalent. So `impl Foo for Vec[T]` and `impl Foo for Vec[U]`
  are duplicates, but `impl Foo for Vec[Int]` and `impl Foo for Vec[T]`
  are not — the first is more specific and *would* be a specialization
  case (§9 forbids it today with E-IMPL-OVERLAP instead of resolving
  it).

### 5.5 No specialization

All overlapping impls produce E-IMPL-OVERLAP at coherence time. The
user must refactor to a single impl with a generic bound.

Reserved syntax: `@[specialize]` on an impl block. Currently rejected
as "not implemented"; reserving signals intent without shipping.

## 6. Implementation Plan

### 6.1 Code map (phases 04a-04c)

| Change | File(s) |
|---|---|
| Argument-position `impl Trait` → synthetic generic param | `resolve/mod.rs:2484-2491` + new helper `impl_trait_to_generic` |
| `Ty::Opaque { def_id, bounds }` | `hir/types.rs:112-115` |
| `FnSignature.hidden_return_ty` | `resolve/symbols.rs:13-19` |
| Body-check fills in hidden type | `typeck/infer.rs` in function-body check |
| Opaque-type identity (two calls → same type) | `typeck/unify.rs` |
| Coherence pass | new `typeck/coherence.rs` |
| Named opaque types (04c) | `parser/ast.rs:707-713`, `resolve/mod.rs:572-580` |
| Formatter round-trip | `formatter/format_type.rs:96-105` |
| Error codes | `diagnostics/` |

### 6.2 Phasing

**Phase 04a — argument-position `impl Trait` + coherence (1 week).**

1. Resolver lowers `Ty::ImplTrait` in argument position to a
   synthetic generic param.
2. Coherence pass runs over all registered impls.
3. Tests: `fn each(items: impl Iterator)` works; overlap produces
   E-IMPL-OVERLAP.

At end of 04a, argument-position `impl Trait` is indistinguishable
from explicit generics. No codegen change.

**Phase 04b — return-position `impl Trait` (2 weeks, blocks on M2).**

4. Add `Ty::Opaque`.
5. Synthesize an opaque when parsing a return-position `impl Trait`.
6. Fill `hidden_return_ty` during body check.
7. Coerce `Ty::Opaque` against declared bounds for method dispatch.
8. Monomorphization uses the hidden type for layout and codegen.
9. Tests: `fn ones -> impl Iterator[Item = Int]` returns a real
   iterator, and `ones.map(|x| x + 1)` type-checks.

**Phase 04c — named opaque types (1 week, optional).**

10. `type FooIter = impl Iterator[Item = Int]` parses.
11. Two functions returning `FooIter` return *the same* opaque type
    — this requires extending `Ty::Opaque { def_id }` to also allow
    a `type_alias_def_id` variant.
12. Motivated only by stdlib storage use cases. Defer if unclear.

## 7. Interactions With Other Tier-2 Features

### 7.1 With associated types (doc 01)

`impl Iterator[Item = Int]` uses the equality-constraint sugar from
doc 01. Phase 04a requires doc 01 phase 01a to have landed.

### 7.2 With GATs (doc 05)

`impl LendingIterator[Item['a] = &'a Str]` — the constraint on the
GAT is parsed via the same sugar. No new logic; GATs simply produce
a projection with a bound lifetime.

### 7.3 With trait objects (doc 06)

`impl Trait` and `dyn Trait` are the two ways to "hide a type."
Static (impl) vs dynamic (dyn). Documentation should make the
difference prominent:

- `impl Trait` — one concrete type, opaque to the caller, zero
  runtime cost beyond monomorphization.
- `dyn Trait` — vtable dispatch, fat pointer, cannot be copied.

### 7.4 With HRTBs (doc 03)

Argument-position `impl for['a] Fn(&'a T) -> U` is legal;
elision covers the common case (doc 03 §5). Return-position
`impl for['a] Fn(&'a T)` is legal but exotic.

### 7.5 With variance (doc 07)

`Ty::Opaque { def_id, bounds }` is *invariant* because the hidden
type is a fixed concrete type, and variance must be conservative on
unknown types. Document.

### 7.6 With specialization (§9)

If specialization ever ships, return-position `impl Trait` gains a
new behaviour: `def foo -> impl Display` in a blanket impl could be
overridden per type. Design for this hook now — `coherence.rs`'s
rejection of overlapping impls becomes a warning-and-pick-most-
specific if a `@[specialize]` attribute is present.

## 8. Phasing

See §6.2.

## 9. Specialization — Why Not to Ship It in v1

### 9.1 The feature

Specialization lets multiple impls of the same trait overlap, with
the compiler picking the *most specific* one at each call site:

```riven
# Blanket impl:
impl[T: Clone] Transform for T
  def transform(self) -> T { self.clone }
end

# Specific impl overrides for Int — picks this one when the concrete
# type is Int.
impl Transform for Int
  def transform(self) -> Int { self * 2 }
end
```

Attractive for:

- efficient fallback implementations (blanket `impl Clone` for
  copyable types that trivially `*x`),
- deriving `Debug` with a specialised format for strings,
- numeric-tower optimisations (`impl Add for Rationals` with
  Rational/Rational fast path).

### 9.2 The cost

Specialization is known to be unsound in the presence of:

- **Lifetimes.** A blanket impl holds for every lifetime; a
  specialised impl holds for some. The "most-specific" rule can pick
  an impl that is strictly less general, changing well-typed code to
  ill-typed code at monomorphization. Rust solved this with
  `always_applicable` predicates and still has open soundness holes
  (rust-lang/rust#40582, open since 2017).
- **Associated types.** A blanket `type Item = Vec[T]` and a
  specialised `type Item = &'static [T]` — a user who wrote code
  against `Self.Item = Vec[T]` now sees a different type at a
  call site that picked the specialised impl.
- **Drop.** A specialised impl with a different Drop behaviour
  changes observable program behaviour silently.

Rust has been trying to stabilize a sound subset (`min_specialization`)
since 2018. As of 2026 it is still `#[feature(min_specialization)]` and
used only inside the compiler and stdlib, behind a `rustc_specialization_trait`
gate. Seven years; still not shipped. This is not a "the Rust team is
slow" problem — it is a "the feature is genuinely hard to make sound"
problem.

### 9.3 The Riven recommendation

- Do not implement specialization in tier 2.
- Coherence rejects overlaps with E-IMPL-OVERLAP. No workaround.
- Reserve `@[specialize]` as an attribute name so it is free for a
  future release.
- Document in the tutorial: "To share code between impls, extract a
  free function or use a helper trait with a `where` bound."

### 9.4 If specialization eventually ships

Minimum viable spec (future):

- Restrict to "non-lifetime specialization": the specialised impl
  must not mention more lifetimes than the generic impl. This dodges
  the lifetime-soundness hole.
- Restrict to "non-associated-type specialization": associated types
  must be identical across impls.
- Marker-trait guards: blanket impls must be tagged with
  `@[specializable]` to opt in.
- Never auto-pick in ambiguous cases — `E-SPEC-AMBIG` if two
  specialised impls both apply.

This is ~4-6 weeks of careful design + coherence work. Deferred.

## 10. Open Questions & Risks

- **OQ-1: opaque type leak through dataflow.** If `def a -> impl
  Iterator` and `def b -> impl Iterator` both exist, can the same
  variable hold either? No — they are different opaque types.
  Document. Error message must say "function `a`'s return type and
  function `b`'s return type are different opaque types, even though
  both implement `Iterator`."
- **OQ-2: can impl Trait be used in a closure return?** `|| -> impl
  Fn()` — probably yes, but the opaque identity must be keyed by the
  closure's def_id. Reject in 04b; revisit.
- **OQ-3: what happens at a recursive call?** `def nested -> impl
  Iterator { if cond { nested() } else { empty_iter } }` — the opaque
  type's body mentions itself. Accept iff the inferred type agrees
  with the declared bounds (normal recursion).
- **OQ-4: does argument-position `impl Trait` allow turbofish?**
  Rust says no — the user can't specify `T` for a synthetic param.
  Riven follows: E-IMPL-ARG-TURBOFISH.
- **R-1: the current coherence hole.** `TraitResolver::register_impl`
  silently overwrites duplicate `(type, trait)` keys
  (`typeck/traits.rs:59-79`). Phase 04a introduces E-IMPL-OVERLAP.
  Existing fixtures must be audited; expect at least the builtin
  iterator-class methods to overlap.
- **R-2: layout regression.** Today `Ty::ImplTrait` layouts to 8
  bytes. Phase 04b introduces `Ty::Opaque`; layout for an opaque is
  the layout of the hidden concrete type. The hidden type isn't known
  until body-check. A layout query before body-check either errors
  (correct but noisy) or returns a conservative placeholder
  (unsound). Recommendation: layout is a post-body pass.
- **R-3: trait inheritance and opaque.** `def f -> impl Iterator`
  inferred body returns `VecIter[T]` which also implements
  `DoubleEndedIterator`. May the caller call `rev()`? Rust says no —
  only the declared bounds are exposed. Riven follows.

## 11. Test Matrix

### 11.1 Positive tests (04a)

- T1: `fn each(items: impl Iterator)` — call with `Vec.new.iter`.
- T2: `fn log(x: impl Displayable + Serializable)` — multi-bound.
- T3: Method call through `impl Trait`: `fn display(x: impl
  Displayable) { x.to_display }`.
- T4: `fn each(items: &impl Iterator)` — reference over impl.

### 11.2 Positive tests (04b)

- T5: `fn ones -> impl Iterator[Item = Int] { (1..).map(|_| 1) }`
  compiles and `ones.take(3).to_vec` returns `[1, 1, 1]`.
- T6: Opaque identity: `let a = ones; let b = ones; a = b` type-
  checks (same opaque type).
- T7: Recursive opaque: `fn rec -> impl Iterator { if ... rec ...
  else empty }` — both arms return the same type.

### 11.3 Negative tests

- N1: Two arms of `if` return different concrete types: `fn bad ->
  impl Iterator { if c then vec!().iter else (1..) }` →
  E-IMPL-RET-MISMATCH.
- N2: Opaque type leaks across two functions: `fn a -> impl Iterator
  { ... } fn b -> impl Iterator { ... } let x = a; x = b` →
  E-OPAQUE-NEQ.
- N3: Two `impl Foo for Int` in the same crate → E-IMPL-DUP.
- N4: Overlapping blanket: `impl[T: Clone] Foo for T; impl Foo for
  Int` → E-IMPL-OVERLAP (phase 04a).
- N5: `impl Trait` used as a field type: `struct S { x: impl
  Iterator }` → E-IMPL-NOT-ALLOWED-HERE. (Use a named opaque type.)
- N6: Turbofish on `impl Trait` param: `each[String](items)` →
  E-IMPL-ARG-TURBOFISH.

### 11.4 Fixture additions

- `tests/fixtures/impl_trait_arg.rvn` — argument-position.
- `tests/fixtures/impl_trait_return.rvn` — return-position, with
  `take.collect`.
- `tests/fixtures/opaque_identity.rvn` — two calls, same type.
- `tests/fixtures/coherence_dup_impl.rvn` — negative.
