# Tier 2.07 — Variance & Subtyping Rules for Lifetimes

Status: Draft (requirements)
Owner: compiler (typeck + borrow_check)
Depends on: existing `LifetimeChecker` (`borrow_check/lifetimes.rs`), existing coercion table (`typeck/coerce.rs`); interacts with tier-2 doc 05 (GATs) and doc 06 (trait objects)
Blocks: soundness of `dyn Trait + 'a` subtyping, GAT lifetime projection, anything that tries to reason about `&'a T` vs `&'b T` when `'b: 'a`

## 1. Summary & Motivation

**Variance** answers one question: *given `'b: 'a` (read "`'b` outlives `'a`"), when is `F<'b>` a subtype of `F<'a>`?* The answer depends on which position the parameter appears in — reference, function argument, function return, interior-mutable cell, `&mut` binding. Rust learned these rules the hard way (soundness holes in early versions of `Cell`, `Fn`-traits, and `HashMap` iterators); Riven should not re-learn them.

Today Riven has:

- An explicit lifetime representation (`Ty::RefLifetime(String, Box<Ty>)`, `Ty::RefMutLifetime(String, Box<Ty>)` — `hir/types.rs:93-95`).
- A lifetime elision checker (`borrow_check/lifetimes.rs:42-98`) implementing Rust's three elision rules and a borrow-outlives-owner check.
- Ad-hoc coercion rules scattered across `typeck/coerce.rs:84-113` and `typeck/unify.rs:262` that *imply* variance decisions (`Option` covariant, `Result` covariant in both parameters, `Vec`/`Hash`/`Set` and `&mut T` invariant "per comment"), but no formal variance table, no variance inference for user structs, and no tests that specifically exercise variance.

What's missing:

- A **formal variance table** for every built-in type constructor.
- A **variance-inference pass** that computes per-parameter variance for user structs, classes, and enums from field types.
- A **subtyping relation** wired into unification so `Ty::RefLifetime('long, T) <: Ty::RefLifetime('short, T)` is accepted in the right places.
- A **marker type** (PhantomData-equivalent) so zero-sized generic parameters can be constrained to a chosen variance.

Without these, substituting lifetimes in nested types becomes unsound the moment anyone writes a type that has both covariant and contravariant uses — and every time an agent extends the coercion table they risk a silent soundness hole.

## 2. Current State

### 2.1 Lifetime representation is split

`Ty` carries both elided references (`Ref(Box<Ty>)`, `RefMut(Box<Ty>)` — no lifetime name) and explicit references (`RefLifetime(String, Box<Ty>)`, `RefMutLifetime(String, Box<Ty>)`) at `hir/types.rs:82-95`. Variance rules must apply to both — meaning an elided lifetime is shorthand for a fresh inference variable, not a "no-lifetime" special case.

The `Display` impl at `hir/types.rs:389-390` prints `&'a T` / `&'a mut T`, confirming the rendering side is ready.

### 2.2 Coercion already makes variance decisions — informally

From `typeck/coerce.rs`:

- Line 84-88: `Option[T] → Option[U]` when `T → U` coerces. This is covariance in `T`.
- Line 90-95: `Result[T, E] → Result[U, F]` when both coerce. Covariant in *both* parameters.
- Line 97-106: `&Ref[T] → &Ref[U]` via `is_subtype_class` — class-inheritance-aware covariance through shared references.
- Line 108-109: **Comments assert** `Vec`, `Hash`, `Set` and `&mut T` are invariant — but the invariance is enforced only by the absence of a coercion rule. There's no test that `&'long mut T` fails to coerce to `&'short mut T` specifically because of variance (they fall through to the `_ => Err(...)` arm).

`Ty::Never` (`!`) is documented as a bottom type at `hir/types.rs:60` ("subtype of everything") but there's no code that actually exercises subtyping with `Never` — it's handled via unification only.

### 2.3 No variance inference for user types

A user-defined `struct Foo { x: &'a T, y: fn(T) }` has *conflicting* variance in `T` — covariant through `x`, contravariant through `y`'s argument, forcing `T` to be invariant overall. Riven's resolver treats all generic parameters uniformly (`resolve/mod.rs` — generic parameter registration carries no variance annotation), and typeck has no pass that walks field types to compute variance. The net effect: user types behave as if **every parameter is invariant**, because the coercion table never recurses into user structs looking for a variance match.

This is *safe* (invariance is the conservative default) but **overly strict**. The moment stdlib ships a `Rc<T>` or `Arc<T>` or `Iter<'a, T>` written in Riven, users will expect `Rc<Child>` → `Rc<Parent>` coercion and won't get it.

### 2.4 No PhantomData equivalent

Types like Rust's `PhantomData<&'a T>` exist precisely to let zero-sized marker types participate in variance inference. Riven has no such type today. Needed once user variance inference ships, because otherwise zero-sized types with phantom parameters (`struct Handle<T> { id: Int }`) default to bivariant — actually unsound — or invariant — usable but imprecise.

### 2.5 Existing lifetime work to build on

- Three-rule elision in `LifetimeChecker::check_elision` (`borrow_check/lifetimes.rs:57-85`).
- Outlives check `check_outlives` at line 101 — only handles scope-relative outlives, not lifetime-parameter relationships.
- `regions.rs` (referenced at line 4) encodes `ScopeId` — Riven uses *scopes* as its region model, not Rust-style lexical lifetime regions. This is simpler but variance rules still need to map onto it.

### 2.6 No fixture exercises variance

Grep across `crates/riven-core/tests/fixtures/` for `'a` or `'static` shows a handful of borrow-check fixtures exist but none test lifetime *subtyping* — the closest is borrow-owner outlives checks. This means today's coercion rules are effectively unchecked for soundness with respect to lifetime variance.

## 3. Goals & Non-Goals

### Goals

G1. Formally document the variance of every built-in type constructor (references, tuples, arrays, `Vec`, `HashMap`, `HashSet`, `Option`, `Result`, function types, closures, `dyn Trait`, `Box`, `Rc`/`Arc` when they land).

G2. Infer variance for user-defined generic types (structs, classes, enums, tuple variants) from field types. Bivariant parameters get reported as warnings with a hint to add a `PhantomData`-equivalent.

G3. Wire variance into the unification/coercion pipeline so valid lifetime subtyping is accepted and invalid subtyping is rejected with a clear diagnostic.

G4. Introduce a `PhantomData[T]` marker type (or attribute equivalent) for users to pin variance on zero-sized parameters.

G5. Ship a test matrix that covers the known-hard variance cases (Rust's historical soundness bugs).

### Non-goals

N1. **User-declared variance annotations** (Scala-style `+T` / `-T`). Riven follows Rust's inference-based approach. Future work.

N2. **Higher-kinded variance** (variance of `F<_>`). Covered implicitly by GATs (doc 05).

N3. **Type-level reasoning for `'static` as a subtype of all lifetimes.** This falls out of G1 + G3 but is called out explicitly because `'static` is the one named lifetime users write by hand.

N4. **Changing Riven's region model from scopes to Rust-style free/bound regions.** That would be a much bigger rewrite; variance rules must live within the existing scope-based model.

## 4. Surface Syntax

Variance is **not user-visible** in v1 — users write generic parameters without annotation and the compiler infers variance:

```
struct Cow[T] {
  data: &'a T,
}
# Compiler infers: T covariant, 'a covariant.
```

The only user-visible surface is the `PhantomData` marker, which lives in the stdlib but is deferred to v1:

```
struct Handle[T] {
  id: Int,
  _phantom: PhantomData[T],
}
# Compiler infers: T invariant (because PhantomData[T] has a &mut T position internally)
# Or, depending on which PhantomData variant:
#   PhantomData[fn(T)]   → contravariant in T
#   PhantomData[fn() -> T] → covariant in T
#   PhantomData[&T]      → covariant in T
#   PhantomData[&mut T]  → invariant in T
```

An optional future extension (non-goal for v1): an attribute

```
@[variance(T = "covariant")]
struct Box[T] { ... }
```

would let stdlib authors override inference for soundness-critical types. Listed here only to show the extension path.

## 5. Variance Inference Rules

### 5.1 The lattice

Four variance values, forming a lattice:

```
        bivariant       (top — any lifetime substitution is safe)
       /          \
covariant     contravariant
       \          /
        invariant        (bottom — only exact equality)
```

Join rules (least upper bound when a parameter appears in multiple positions):

| join | cov | contra | inv | biv |
|------|-----|--------|-----|-----|
| **cov**    | cov | inv    | inv | cov |
| **contra** | inv | contra | inv | contra |
| **inv**    | inv | inv    | inv | inv |
| **biv**    | cov | contra | inv | biv |

Invariant dominates.

### 5.2 Built-in type-constructor variance (the "ground truth" table)

| Type | Variance in parameters |
|------|-----------------------|
| `&'a T` | covariant in `'a`, covariant in `T` |
| `&'a mut T` | covariant in `'a`, **invariant** in `T` |
| `Box[T]` | covariant in `T` |
| `Rc[T]`, `Arc[T]` (future) | covariant in `T` |
| `Cell[T]`, `RefCell[T]` (future) | **invariant** in `T` |
| `[T; N]` (array) | covariant in `T` |
| `Vec[T]` | **invariant** in `T` |
| `HashMap[K, V]` | **invariant** in `K`, **invariant** in `V` |
| `HashSet[T]` | **invariant** in `T` |
| `Option[T]` | covariant in `T` |
| `Result[T, E]` | covariant in `T`, covariant in `E` |
| `(T1, T2, ..., Tn)` tuple | covariant in each `Ti` |
| `fn(A1, ..., An) -> R` | **contravariant** in each `Ai`, covariant in `R` |
| `Fn`/`FnMut`/`FnOnce` trait objects | same as function types |
| `dyn Trait + 'a` | covariant in `'a`; trait args inherit the trait's declared variance (future — currently treat as invariant) |
| `*const T` (raw pointer) | covariant in `T` |
| `*mut T` (raw pointer) | **invariant** in `T` |
| `PhantomData[T]` | covariant in `T` (use `PhantomData[fn(T)]` etc. for others) |

**Rationale for the non-obvious cases:**

- `&mut T` invariant in `T`: the classic soundness proof. If `&mut T` were covariant in `T` and `Child <: Parent`, then `&mut Child` would be usable as `&mut Parent`, and writing a `Parent` through it would leave an invalid `Child` behind.
- `Vec[T]` and `HashMap[K, V]` invariant: because `Vec` exposes `&mut` to its elements, even though the `Vec` value itself is owned.
- `fn(A) -> R` contravariant in `A`: a function accepting `Parent` can be used wherever a function accepting `Child` is expected, because `Child <: Parent` means every `Child` is a valid `Parent` — so the function will accept it.

### 5.3 Inference algorithm for user types

Run after resolve, before the main typeck pass:

```
fn infer_variance(def_id: DefId) -> Map<ParamIdx, Variance> {
  # Start: every parameter is bivariant (top).
  let mut result = bivariant_map(def_id.generic_params);

  # Fixed-point iteration — needed because user types can be recursive.
  loop {
    let before = result.clone();
    for field in def_id.fields() {
      visit_ty(&field.ty, Variance::Covariant, &mut result);
    }
    if result == before { break; }
  }
  result
}

fn visit_ty(ty: &Ty, current: Variance, acc: &mut Map<ParamIdx, Variance>) {
  match ty {
    Ty::GenericParam(idx) => acc[idx] = acc[idx].join(current),
    Ty::RefLifetime(_, inner) | Ty::Ref(inner) => visit_ty(inner, current, acc),
    Ty::RefMutLifetime(_, inner) | Ty::RefMut(inner) =>
      visit_ty(inner, Variance::Invariant, acc),
    Ty::Fn { params, ret } => {
      for p in params { visit_ty(p, current.flip(), acc); }  # contravariant
      visit_ty(ret, current, acc);
    }
    Ty::Vec(inner) | Ty::HashSet(inner) =>
      visit_ty(inner, Variance::Invariant, acc),
    Ty::HashMap(k, v) => {
      visit_ty(k, Variance::Invariant, acc);
      visit_ty(v, Variance::Invariant, acc);
    }
    Ty::Option(inner) | Ty::Box(inner) | Ty::Array(inner, _) =>
      visit_ty(inner, current, acc),
    Ty::Result(ok, err) => {
      visit_ty(ok, current, acc);
      visit_ty(err, current, acc);
    }
    Ty::Tuple(elems) => for e in elems { visit_ty(e, current, acc); },
    Ty::Struct { def_id: other, generic_args } | Ty::Class { ... } | Ty::Enum { ... } => {
      # Use the already-computed variance of `other` (or assume invariant if cyclic).
      let other_variance = lookup_or_default_invariant(other);
      for (arg, arg_variance) in generic_args.zip(other_variance) {
        visit_ty(arg, current.compose(arg_variance), acc);
      }
    }
    _ => {}  # primitives, never, etc.
  }
}
```

`Variance::flip` is `covariant ↔ contravariant`, fixing invariant and bivariant. `Variance::compose` is transitive: `covariant.compose(X) = X`; `contravariant.compose(X) = X.flip()`; `invariant.compose(_) = invariant`; `bivariant.compose(_) = bivariant`.

### 5.4 Bivariant parameters → warning

After inference, any parameter that remains bivariant means it's only used in positions that don't observe it (typically a phantom parameter). Emit warning `W0710: generic parameter 'T' is unused in variance-relevant positions; use PhantomData[T] to pin variance` (or `@[phantom]` attribute — see §OQ-2).

### 5.5 Subtyping with lifetimes

Given computed variances, the subtyping relation `Ty::RefLifetime('long, T)` `<:` `Ty::RefLifetime('short, T)` (where `'long: 'short`) holds iff:

- for a covariant param position: substitute `'long` for `'short` freely;
- for a contravariant param position: substitute only in the opposite direction;
- for an invariant param: require exact equality of lifetimes (only structural unification, no outlives weakening);
- for a bivariant param: accept any substitution.

The existing `LifetimeChecker::check_outlives` at `borrow_check/lifetimes.rs:101` proves `'long: 'short` when `'long`'s scope *contains* `'short`'s scope. Variance-aware subtyping consumes that proof.

## 6. Inference vs Annotation

**Recommendation: inference only in v1.** Rationale:

1. **User ergonomics.** Scala/C# require `+T`/`-T` on every parameter; even their standard libraries get it wrong regularly. Rust's inference lets beginners write `struct Wrapper<T> { x: T }` without understanding variance at all.
2. **Marker-type ergonomics cover the edge cases.** When users actually need to *override* inference (rare, and always for unsafe-code or FFI soundness), `PhantomData` variants are sufficient.
3. **Error-message quality.** Variance errors are already hard; adding a user-declared annotation means the compiler has to explain both *your declaration* and *the inferred truth* mismatching.

The cost is one new compiler pass. It's cheap — Rust's `rustc_typeck::variance` is under 500 lines in total.

Risk: once inference is wired in, changing a private field type can silently change a public generic's variance (a semver hazard). Mitigate by a lint `W0711: public type's inferred variance changed` that the versioning tooling (tier-5 doc 02) can enforce at edition boundaries.

## 7. Implementation Plan

### Files to create

- `crates/riven-core/src/typeck/variance.rs` — the inference pass. Exposes `VarianceTable` keyed on `DefId`.
- `crates/riven-core/src/hir/variance.rs` (if the shared type is needed by more than one crate) — the `Variance` enum + lattice operations.

### Files to modify

- `hir/types.rs`: add `Ty::PhantomData(Box<Ty>)` (or `Ty::Phantom { witness: Box<Ty> }` to keep the convention open). Cite: around line 95 alongside the reference types.
- `resolve/mod.rs`: register `PhantomData` as a built-in type constructor alongside `Vec`/`HashMap`/`HashSet`.
- `typeck/mod.rs`: run `variance::infer_all` after symbol collection, before the main inference loop. Store the resulting `VarianceTable` on `TypeckContext`.
- `typeck/unify.rs:262` and neighboring — replace the ad-hoc "Option covariance" block with a single dispatch through the variance table.
- `typeck/coerce.rs:60-113`: delete the per-constructor rules and replace with a variance-driven walk. The `is_subtype_class` helper at line 117 stays — it handles nominal class inheritance, which is orthogonal to parametric variance.
- `borrow_check/lifetimes.rs`: add `check_lifetime_subtype(long: &Ty, short: &Ty) -> Result<(), LifetimeError>` that consumes the `VarianceTable`.
- `diagnostics`: add error codes `E0705` *invariant lifetime mismatch* and `E0706` *contravariant lifetime mismatch* (see tier-5 doc 04 for the namespace).

### Test coverage to add

- Fixture: `fixtures/variance/option_covariant_lifetime.rvn` — `Option[&'long T]` flows into `Option[&'short T]`.
- Fixture: `fixtures/variance/refmut_invariant.rvn` — `&'long mut T` must *not* coerce to `&'short mut T`; expect E0705.
- Fixture: `fixtures/variance/fn_contravariant_arg.rvn` — `fn(&'short T)` coerces to `fn(&'long T)` (the function accepting a narrower arg works where a wider one is expected).
- Unit tests in `typeck/variance.rs` for the inference algorithm using synthetic `DefId`s.
- Proptest: generate random struct definitions with varying field shapes and assert that the fixed-point iteration converges in O(depth) steps.

## 8. Interaction With Other Tier-2 Features

### 8.1 GATs (doc 05)

`type Iter<'a>: Iterator<Item = &'a T>` — the associated type's lifetime parameter introduces per-projection variance. The inference algorithm must treat `<Self as Iterable>::Iter<'a>` as a type-constructor application and look up the variance of the associated type. Practical effect: the variance table keys on `(DefId, AssocName)` pairs as well as plain `DefId`s.

### 8.2 Trait objects (doc 06)

`dyn Trait + 'a` is covariant in `'a`. For the trait's own generic parameters, object-safety rules already restrict what's expressible — but where generic trait args are allowed (`dyn Iterator<Item = T>`), the variance is the trait's declared/inferred variance of that parameter. Mark this in doc 06's §objectsafety as an open dependency on the variance doc.

### 8.3 HRTBs (doc 03)

`for<'a> Fn(&'a T) -> &'a T` — variance of `'a` is *bound* (quantified), not applied. The variance table is irrelevant for bound lifetimes; the check reduces to unification under a fresh skolem. Bake this into the HRTB doc's matching algorithm.

### 8.4 Associated types (doc 01)

Normalized projections `<T as Trait>::Output` inherit the variance of the trait's declaration for `Output`. Until associated types have declarable variance (future), treat projections as invariant in all args — conservative but safe.

## 9. Phasing

**Phase 7a — document and test what exists (1 week).** Move the inline comments in `coerce.rs:108-109` into a formalized table in `hir/types.rs` (a doc comment on the relevant `Ty` variants). Add fixtures for the existing covariance/invariance expectations so any future refactor can't silently change behavior.

**Phase 7b — variance inference for user types (2 weeks).** Implement `typeck::variance` with the fixed-point algorithm. Store a `VarianceTable` on `TypeckContext`. Do *not* yet change coercion/unification behavior — the pass runs but its result is only consumed by new fixture-check code.

**Phase 7c — wire variance into coercion/unification (2 weeks).** Replace the ad-hoc rules at `coerce.rs:84-109` with table-driven dispatch. Add `E0705`/`E0706` diagnostics. Add the `PhantomData` marker type. Update stdlib (once tier-1 is ready) to add `PhantomData` fields to collections that need them.

**Phase 7d — bivariant warning + semver lint (1 week, after tier-3 LSP).** Wire `W0710` (bivariant unused param) and `W0711` (variance-changed-across-edition) into the diagnostic pipeline. Part of the tier-5 stability story.

Total ~6 weeks linear, 3-4 weeks if 7c and the PhantomData work parallelize with stdlib tier-1.

## 10. Open Questions & Risks

### Open questions

**OQ-1.** **`Never` (`!`) and subtyping.** `hir/types.rs:60` claims `!` is a bottom type, subtype of everything. Current code handles this via unification only. Should we formalize it in the variance framework (treat `!` as inhabiting every type position) or leave the ad-hoc handling? Recommend: leave ad-hoc; variance is about *parametric* substitution, bottom types are orthogonal.

**OQ-2.** **`PhantomData` vs `@[phantom(...)]` attribute.** Rust uses the type; Swift has no equivalent because it doesn't need one (variance via protocol). Riven could ship either. Recommend `PhantomData[T]` for Rust familiarity, but keep the attribute form as an escape hatch for stdlib authors who want declarative variance without a field.

**OQ-3.** **`'static` as the supertype of all lifetimes.** Falls out of the framework because `'static` outlives every scope. Confirm this works in practice with the scope-based region model — may need a special-case in `check_outlives`.

**OQ-4.** **Scope-based regions vs free lifetime variables.** Riven's `regions.rs` encodes scopes, not free/bound lifetime variables. Variance works for *parameters* (which are always free within a definition) but GATs/HRTBs introduce bound lifetimes. A follow-up doc may need to extend the region model. Call out as future work.

**OQ-5.** **Public-API variance semver.** Can a patch release silently change a public type's inferred variance? Recommend: treat inferred variance as part of the public API and gate changes on edition boundaries (tier-5 doc 02).

### Risks

**R1. Fixed-point non-termination for recursive types.** `struct List[T] { next: Option[Box[List[T]]] }` — the inference pass depends on itself. Standard fix: seed all parameters with bivariant and iterate; the lattice is finite, so convergence is guaranteed.

**R2. Silently breaking existing code when the coercion rewrite lands.** Phase 7c replaces the current ad-hoc rules with table-driven logic; if the table doesn't exactly match today's behavior, fixture tests fail. Mitigate by shipping phase 7a's fixtures *before* phase 7c.

**R3. `&mut T` invariance breaking intuitive-looking code.** Many users expect `&mut Child` to coerce to `&mut Parent` because single-value mutation seems safe. It isn't. Ensure `E0705` has a `help:` note explaining the soundness rationale (tier-5 doc 05).

**R4. PhantomData ergonomic friction.** Users rarely write zero-sized phantom types in practice, but every FFI wrapper needs them. Keep the W0710 warning actionable: the diagnostic should suggest the exact `PhantomData` variant.

**R5. Class inheritance + parametric variance interaction.** `is_subtype_class` at `coerce.rs:117` already handles nominal class inheritance. When a generic class like `Container[T] extends AbstractContainer[T]` has inheritance *plus* variance, the two rules compose. Current behavior: only one of the two fires because variance isn't checked through user types. After phase 7c both will fire; add a fixture that exercises `Container[Child] <: AbstractContainer[Parent]` via nominal + covariance.

## 11. Test Matrix

| # | Case | Expected |
|---|------|----------|
| V1 | `&'long T` → `&'short T` where `'long: 'short` | ✅ coerces |
| V2 | `&'long mut T` → `&'short mut T` | ❌ E0705 (mut invariant in lifetime is false — mut is covariant in lifetime, invariant in T) |
| V3 | `&'a mut Child` → `&'a mut Parent` | ❌ E0705 (invariant in inner `T`) |
| V4 | `Option[&'long T]` → `Option[&'short T]` | ✅ covariance passes through |
| V5 | `Vec[&'long T]` → `Vec[&'short T]` | ❌ E0705 (Vec invariant in T) |
| V6 | `fn(&'short T) -> ()` → `fn(&'long T) -> ()` | ✅ contravariance of arg |
| V7 | `fn(&'long T) -> ()` → `fn(&'short T) -> ()` | ❌ E0706 |
| V8 | `fn() -> &'long T` → `fn() -> &'short T` | ✅ covariance of return |
| V9 | `(T1, T2)` tuple with T1 covariant + T2 covariant | ✅ |
| V10 | `HashMap[&'long K, V]` → `HashMap[&'short K, V]` | ❌ E0705 (HashMap invariant in K) |
| V11 | Recursive `List[T]` inference converges | ✅ |
| V12 | `Cell[T]` (future) `Cell[Child]` → `Cell[Parent]` | ❌ E0705 (interior mutability → invariant) |
| V13 | `Box[Child]` → `Box[Parent]` | ✅ covariance |
| V14 | `dyn Trait + 'long` → `dyn Trait + 'short` | ✅ covariance of lifetime |
| V15 | `*mut T` vs `*const T` variance | `*mut` invariant in T, `*const` covariant |
| V16 | `struct Wrapper[T] { x: T }` inferred covariant | ✅ matches phase 7b output |
| V17 | `struct MutWrapper[T] { x: &mut T }` inferred invariant | ✅ |
| V18 | `struct FnWrapper[T] { f: fn(T) }` inferred contravariant | ✅ |
| V19 | `struct Conflict[T] { a: T, b: fn(T) }` inferred invariant (cov ⊔ contra = inv) | ✅ |
| V20 | `struct Unused[T] { id: Int }` triggers W0710 | ✅ warning |
| V21 | `struct Marked[T] { id: Int, _p: PhantomData[T] }` inferred covariant, no warning | ✅ |
| V22 | Class inheritance + generics: `Child[T]: Parent[T]` covariance propagates | ✅ |
| V23 | `'static` as supertype — `fn() -> &'static T` flows into `fn() -> &'any T` | ✅ |

## 12. Acceptance Criteria

- Variance inference pass runs on every user-defined generic type and memoizes results.
- All 23 test-matrix cases pass.
- The existing `coerce.rs:84-109` ad-hoc rules are replaced by a single table-driven dispatch that passes all existing coercion fixtures *without* changes.
- `W0710` fires on any user struct with a truly-unused generic parameter, and the suggested `PhantomData` fix is actionable.
- Documentation in the language reference (tier-5 doc 01) contains the §5.2 variance table verbatim, with normative status.

## Appendix A — File Citations

| Path | Line(s) | Relevance |
|------|---------|-----------|
| `crates/riven-core/src/hir/types.rs` | 60 | `Never` bottom-type note |
| | 82-95 | `Ref` / `RefLifetime` / `RefMut` / `RefMutLifetime` |
| | 389-390 | `Display` for explicit-lifetime references |
| `crates/riven-core/src/typeck/coerce.rs` | 84-88 | current `Option` covariance |
| | 90-95 | current `Result` covariance on both params |
| | 97-106 | `&Ref` subtype via class inheritance |
| | 108-109 | informal invariance comments for `Vec`/`Hash`/`Set`/`&mut T` |
| | 117-145 | `is_subtype_class` |
| `crates/riven-core/src/typeck/unify.rs` | 262 | duplicated `Option` covariance |
| `crates/riven-core/src/borrow_check/lifetimes.rs` | 34-85 | `LifetimeChecker` + three-rule elision |
| | 101 | `check_outlives` (scope-based) |
| `crates/riven-core/src/borrow_check/regions.rs` | (entire) | `ScopeId` region model |
| `crates/riven-core/src/resolve/mod.rs` | ~200 | built-in type-constructor registration — where `PhantomData` will go |
| `crates/riven-core/src/borrow_check/errors.rs` | 6-16 | existing `ErrorCode` range — `E0705`/`E0706` reserved here |
