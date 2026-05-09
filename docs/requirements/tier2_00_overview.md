# Tier 2 Overview — Type-System Extensions

Companion index for the seven Tier-2 requirements documents. Read this
before the individual docs.

## The docs

| # | Feature | Doc |
|---|---------|-----|
| 01 | Associated types | [tier2_01_assoc_types.md](tier2_01_assoc_types.md) |
| 02 | Const generics | [tier2_02_const_generics.md](tier2_02_const_generics.md) |
| 03 | Higher-ranked trait bounds (HRTBs) | [tier2_03_hrtbs.md](tier2_03_hrtbs.md) |
| 04 | `impl Trait` (argument/return) + specialization | [tier2_04_impl_trait_and_specialization.md](tier2_04_impl_trait_and_specialization.md) |
| 05 | Generic associated types (GATs) | [tier2_05_gats.md](tier2_05_gats.md) |
| 06 | Trait objects (`dyn Trait`) | [tier2_06_trait_objects.md](tier2_06_trait_objects.md) |
| 07 | Variance & subtyping | [tier2_07_variance.md](tier2_07_variance.md) |

## Scope and framing

Tier-1 closes the gap between the advertised language (tutorials, fixture
programs) and what compiles-and-runs correctly today. Tier-2 closes the
gap between "correct on concrete types" and "correct on parametric types."
None of this is optional if Riven intends to ship an `Iterator` trait, a
stdlib that returns `impl Iterator` from `Vec.iter`, a vtable-backed
plugin / scripting boundary, or safe lifetime subtyping on closures and
references.

The seven features in this tier cluster into three groups:

1. **Trait extensions that change `Ty` and `TraitRef`.** Associated types
   (01), GATs (05), HRTBs (03). These change what a trait signature can
   mention and how the solver instantiates it.
2. **Dispatch and opacity.** `impl Trait` (return-position, argument-
   position) and specialization (04); trait objects with an object-safety
   checker and vtable layout (06). These are about how a generic use-site
   gets turned into code.
3. **Subtyping.** Variance and lifetime subtyping (07). This is the
   "unseen" machinery that allows `&'static T` to flow into a `&'a T`
   context — currently absent and deferred.

## Pre-existing state (what already exists in the tree)

The compiler has *partial* scaffolding for every Tier-2 feature except
variance and HRTBs. The scaffolding is load-bearing for the design in
these docs; here is where it lives:

- **Trait `type` items parse into `TraitItem::AssocType`.**
  `crates/riven-core/src/parser/mod.rs:908-914`. The HIR variant
  `HirTraitItem::AssocType` exists (`hir/nodes.rs:483-486`) and resolve
  walks the AST into it (`resolve/mod.rs:940-945`). `TraitInfo::
  assoc_types: Vec<String>` is stored (`resolve/symbols.rs:65`). **No
  typeck, no projection, no `Self::Item` anywhere.**

- **`impl` blocks accept `type Name = T` items.**
  `parser/mod.rs:1136-1149`, `resolve/mod.rs:721-728` and `1024-1033`.
  The HIR is built (`HirImplItem::AssocType` at `hir/nodes.rs:511-515`)
  and the MIR lowerer trivially skips the item (`mir/lower.rs:107`).
  Again: nothing downstream uses the projection.

- **`Ty::ImplTrait(Vec<TraitRef>)` and `Ty::DynTrait(Vec<TraitRef>)` both
  exist.** `hir/types.rs:112-115`. Parsing is live
  (`parser/types.rs:47-51, 205-218`). Resolution produces these variants
  (`resolve/mod.rs:2484-2499`). Layout treats `DynTrait` as a 16-byte fat
  pointer (`codegen/layout.rs:333-335`) and `ImplTrait` as a pointer-sized
  placeholder (`:337-339`). **No vtable emission, no object-safety check,
  no structural-vs-nominal dispatch switch.**

- **`Ty::Array(Box<Ty>, usize)` stores a const-evaluated size.**
  `hir/types.rs:73`. The AST carries `size: Option<Box<Expr>>`
  (`parser/ast.rs:66-70`), and resolve constant-folds small integer
  literals into the `usize` slot but gives up on anything more
  interesting (`resolve/mod.rs:2470-2472`, look for the `_ => 0` fallback).
  **No generic const parameters, no `[T; N]` with `N` bound at a
  function signature.**

- **Lifetime tokens parse; lifetime generic-params resolve to nothing.**
  `parser/types.rs:360-368` accepts `'a`. `resolve/mod.rs:2679-2682`
  explicitly drops them on the floor ("Lifetimes are tracked but not
  yet used in Phase 3"). `borrow_check/lifetimes.rs` implements elision
  rules (rule 1, 2, 3) and a return-ref-to-local check but has **no
  named-lifetime data structure and no subtyping relation.**

- **One piece of variance already exists.** `typeck/coerce.rs:84-95`
  covers `Option[&Child] → Option[&Parent]` and `Result[&Child, E] →
  Result[&Parent, E]` via the class-inheritance subtype relation
  (`is_subtype_class` at `coerce.rs:117-150`). Unification extends the
  same pattern (`typeck/unify.rs:262`). These are *ad hoc* coercions;
  there is no `Variance` enum, no per-parameter annotation, and no
  inference. See §§2 and 5 of the variance doc.

- **Type monomorphization does not exist.** The runtime passes every
  generic slot as `int64_t` (`runtime/runtime.c:220-225`, referenced by
  tier1_00 §B4 and §R5). Cranelift maps `TypeParam`, `Infer`, `DynTrait`,
  `ImplTrait`, `Class`, `Enum`, `Tuple`, `Array` — everything
  non-primitive — to `I64` (`codegen/cranelift.rs:965-984`). **This is
  the defining constraint on Tier-2.** See the dependency section below.

## Dependency on int64-slot erasure

Every Tier-2 feature lands in one of three buckets with respect to the
current "everything is an I64" codegen:

- **Bucket A (can ship without changing the runtime):** trait objects
  (06) — already planned as a fat pointer; const generics (02) if
  restricted to type-level integers that do not affect layout (e.g.,
  `Array[T, N]` where `N` just indexes field offsets computed at
  compile-time); variance (07) which is a type-check-time relation only.

- **Bucket B (requires real monomorphization — i.e., generating one
  compiled function per type instantiation):** associated types (01)
  where the associated type is used as a concrete field or local
  (e.g., `type Item = T` and then `let x: Self.Item`); GATs (05);
  specialization (04) where the whole point is to pick a different code
  path for a different `T`; `impl Trait` (04) in return position when
  the opaque type is not pointer-sized (e.g., `impl Iterator` returning
  a struct).

- **Bucket C (ambiguous — depends on implementation detail):** HRTBs
  (03) — the lifetime parameter itself is erased at codegen, so an HRTB
  bound on a closure `for<'a> Fn(&'a T) -> &'a T` does *not* require
  monomorphization; but HRTBs showing up in `impl Trait` on a struct
  field might, by transitivity.

**Decision: monomorphization must be built as part of Tier-2.** The
alternative — erasing all generic types to I64 — caps the stdlib at
pointer-sized element types and already leaks silently on tuples and
structs larger than 8 bytes (tier1_01 §2.1 note at line 29). A
well-specified monomorphization pass simultaneously unblocks:

1. the tier-1 stdlib (which needs `HashMap[K, V]` where `V` is not
   pointer-sized),
2. associated types as concrete types,
3. GATs,
4. specialization,
5. `impl Trait` in return position with non-pointer-sized opaque types.

A dedicated tier-2 **Monomorphization** phase is specified in doc 01
§Phasing and referenced from the other docs. Until it lands, the only
tier-2 features that can ship are: trait objects (doc 06), variance
(doc 07), and the type-check front-end of associated types and HRTBs
(parsing and elaboration, no codegen surface change).

## Recommended implementation order

```
           ┌──────────────────────────────────┐
           │ M0 pre-flight: named lifetimes   │
           │      (region solver rewrite)     │
           └────────────┬─────────────────────┘
                        ▼
           ┌──────────────────────────────────┐
           │ M1  Variance (doc 07)            │
           │     — type-check only,           │
           │       no codegen                 │
           └────────────┬─────────────────────┘
                        ▼
           ┌──────────────────────────────────┐
           │ M2  Monomorphization pass        │
           │     (new: mir/monomorphize.rs)   │
           │     + drop-I64-erasure           │
           └────────────┬─────────────────────┘
          ┌─────────────┼─────────────┐
          ▼             ▼             ▼
     ┌─────────┐  ┌─────────┐  ┌──────────────┐
     │ M3  Assoc│  │ M4 Const│  │ M5  Trait    │
     │   types  │  │ generics│  │     objects  │
     │  (01)    │  │ (02)    │  │    (06)      │
     └─────┬────┘  └─────────┘  └──────────────┘
           ▼
     ┌──────────────────────┐
     │ M6  HRTBs (03)       │
     │     (closures first) │
     └────────┬─────────────┘
              ▼
     ┌──────────────────────┐
     │ M7  GATs (05)        │
     └────────┬─────────────┘
              ▼
     ┌──────────────────────────────────┐
     │ M8  impl Trait (return-pos) (04) │
     └────────┬─────────────────────────┘
              ▼
     ┌──────────────────────────────────┐
     │ M9  Specialization (04)          │
     │     — optional, see §open        │
     └──────────────────────────────────┘
```

Key cross-feature dependencies:

- **Associated types (01) must come before Iterator-based stdlib.**
  `Vec.iter` returns `impl Iterator` whose `Item = &T`. Without
  `type Item` on the trait, `Iterator::next` cannot be expressed,
  which cascades into `.map`/`.filter`/`.collect` being untyped
  (currently noops — tier1 B4). This is the single largest
  dependency in the graph.

- **GATs (05) require assoc types (01).** Obvious — GATs are just
  assoc types with their own generic parameters. Not worth revisiting
  the solver twice; however, (05) can lag (01) by a full release
  because the GAT-dependent stdlib surface (`LendingIterator`,
  `StreamingIterator`) is not on the tier-1 path.

- **HRTBs (03) benefit enormously from lifetime elision.** Riven's
  elision rules already cover the 80% case
  (`borrow_check/lifetimes.rs:55-85`). HRTBs are only *needed* when a
  closure outlives the function that takes it and must work for any
  input lifetime — e.g., `def chunk_each[F: for<'a> Fn(&'a Slice)](f: F)`.
  **Picking good elision rules first can move HRTBs from "required"
  to "advanced-only."** See doc 03 §5.

- **Trait objects (06) are independent of monomorphization and can be
  shipped first.** `dyn Trait` is a uniform fat pointer; the codegen
  change is localised (layout already assigns 16 bytes) and the type-
  check work is the object-safety rule set. See doc 06 §Phasing.

- **Variance (07) is a prerequisite for sound subtyping with named
  lifetimes.** Until variance exists, any `&'long T` → `&'short T`
  coercion is either accepted by accident (via ad-hoc rules like
  `Option` covariance) or rejected outright, with no way to express
  the intended subtype direction. This *must* precede assoc-types-with-
  lifetime-references.

- **Specialization (04) is the highest-risk feature in the tier.**
  Rust shipped `min_specialization` in 2018 and still has not
  stabilised it after 7+ years of unsound interaction with traits + Drop.
  **Recommendation: omit specialization from v1.** Ship `impl Trait`
  (the useful half of doc 04) and defer specialization behind a feature
  flag to a later release. See doc 04 §9 for the reasoning.

## Cross-tier dependency on Tier-1

- Tier-1 B1 (Drop codegen) must land before doc 01 phase 2 (associated
  types as concrete fields). Without real Drop, a `type Item = String`
  projection inside a `Vec[Item]` field leaks silently; with Drop, it
  drops correctly through existing drop-glue per-instantiation once M2
  monomorphization is in.
- Tier-1 B2 (derive untangling) is a prerequisite for auto-derived
  vtable glue (doc 06 §6.4 auto-derive of `DynSafe`). Derive metadata
  must live in `ClassInfo` / `EnumInfo`, not in the one-off
  `StructInfo.derive_traits`.
- Tier-1 doc 04 phase 4a (Drop trait registration) is strictly
  required before variance (doc 07), because drop-check and variance
  interact on lifetime parameters in phantom-data-shaped types (doc 07
  §7).

## Open questions for the project lead (cross-cutting)

These surfaced in multiple docs and need a single ruling before tier-2
implementation starts.

1. **Monomorphization strategy.** Whole-program monomorphization (one
   function per instantiation, like Rust's `-Clto`) vs lazy / on-demand
   (emit instantiations as the call graph expands). Doc 01 §Phasing
   recommends whole-program for simplicity; Cranelift already emits per-
   function. This doubles compile time for heavy-generic code but is
   simpler to implement and debug. Decide before M2.

2. **Projection syntax: `Self::Item` vs `Self.Item`.** Ruby uses `.` for
   method dispatch and `::` is free for type-level operations; Rust uses
   `::` everywhere. The tutorial (`docs/tutorial/08-traits.md:30-33`) uses
   `Self.Item`, matching Ruby. But `Self.Item` conflicts visually with
   field access on a `Self` value. Docs 01, 05, 06 all need the same
   answer. Recommendation: `Self.Item` at the type level, consistent with
   Riven's dotted module paths; distinguish from expression-level
   `self.item` by Capitalisation of `Self`. (The parser already reads
   uppercase `TypeIdentifier` differently from lowercase `Identifier` —
   `lexer/mod.rs`.)

3. **Const-generic evaluator scope.** Doc 02 proposes evaluating
   expressions like `N + 1`, `N * 2`, and `N * M` at compile time for
   array sizes. Zig's lesson is that once you open `comptime`, users
   want recursion, loops, and arbitrary procedures. Decision needed:
   tier-2 const generics accept (a) only integer literals and generic
   parameter references (no arithmetic); (b) integer literals plus
   `+ - * /` on generic params; (c) anything `const fn` returns. (a)
   is the minimum viable; (b) matches Rust's stable subset; (c) is
   out of scope.

4. **Variance: inferred or declared?** Rust infers variance from
   structural use. Scala declares it (`+T` / `-T`). Declared variance
   forces users to reason about it at every generic definition and is
   verbose but auditable; inferred variance is invisible and sometimes
   surprising. Doc 07 §6 recommends inferred with an optional `@[covariant]`
   / `@[contravariant]` escape hatch for documentation. Confirm.

5. **Ship specialization at all in v1?** Doc 04 §9 argues against.
   Recommendation: no. Confirm.

6. **Object-safety: what exactly is forbidden?** Doc 06 §4 enumerates
   seven rules. Most match Rust's list. Two are Riven-specific:
   (a) must a trait be `Sized`-like? Riven has no unsized types, so yes
   by fiat. (b) what about methods with `consume self`? Rust forbids
   them from vtables; Riven should too. Confirm.

7. **HRTB surface syntax.** Doc 03 §4 proposes `for['a] Fn(&'a T) -> &'a T`,
   reusing Riven's bracket-generic syntax. Rust uses `for<'a>`. The
   former is consistent with `[T, U]` for generics; the latter is a
   visual clue that HRTBs are special. Pick one and document in the
   tutorial.

## Total estimate

- M0 pre-flight (named lifetimes): 2 weeks
- M1 variance: 2 weeks
- M2 monomorphization: 4-6 weeks (the big one)
- M3 assoc types: 3 weeks
- M4 const generics: 2-3 weeks
- M5 trait objects: 2-3 weeks (independent; can run in parallel with M2)
- M6 HRTBs: 2 weeks
- M7 GATs: 3-4 weeks
- M8 impl Trait: 2 weeks
- M9 specialization: 4-6 weeks (recommended: skip)

Sequential critical path (excluding skip-ables): **~24-31 weeks.**
With M5 (trait objects) and M1 (variance) parallelised against M2, the
walltime drops to ~20-26 weeks.

## Conventions (same as Tier-1)

- File:line citations in the form `crates/riven-core/src/foo.rs:123`.
- Per-feature phasing in a `Phasing` section with 2-letter phase codes
  (e.g., `01a`, `01b`).
- Riven code examples inside fenced blocks tagged `riven` (the tutorial
  uses this convention too — `docs/tutorial/08-traits.md:7`).
- "Open questions" surfaced per-doc, then rolled up here.
- Error codes use `E-<FEATURE>-<SHORTNAME>` form, e.g., `E-ASSOC-AMBIG`,
  `E-DYN-NOT-SAFE`.
