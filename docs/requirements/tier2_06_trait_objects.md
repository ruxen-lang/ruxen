# Tier 2.06 — Trait Objects (`dyn Trait`)

Status: Draft (requirements)
Owner: compiler
Depends on: tier-1 doc 04 (Drop) for vtable drop slot; associated types
            (doc 01) for projection-safety rule
Blocks: plugin/script interfaces; heterogeneous collections
        (`Vec[Box[dyn Drawable]]`); event-driven code

## 1. Summary & Motivation

A *trait object* erases the concrete type of a value behind a trait:
`dyn Trait` is a runtime-dispatched view through which you can call
the trait's methods without knowing the underlying type. The canonical
use cases:

1. **Heterogeneous collections.** `Vec[Box[dyn Drawable]]` holds
   `Square`, `Circle`, `Triangle` in the same vector.
2. **Plugin boundaries.** Loaded-at-runtime modules expose a
   `dyn Command` interface; the runtime doesn't know the concrete
   type.
3. **Callbacks stored in fields.** `on_click: Box[dyn Fn()]`.
4. **Type-erased error types.** `Box[dyn Error]` — a common stdlib
   pattern.

The compiler already has most of the type-level scaffolding: `Ty::DynTrait(Vec<TraitRef>)`
(`hir/types.rs:115`), layout as 16-byte fat pointer
(`codegen/layout.rs:333-335`). What's missing: vtable emission,
object-safety checking, method-call lowering to vtable dispatch,
drop-via-vtable. This doc specifies the complete set of changes to
ship `dyn Trait` end-to-end.

## 2. Current State

### 2.1 Type, parse, layout all exist; nothing else

- `Ty::DynTrait(Vec<TraitRef>)` — `hir/types.rs:112-115`.
- Parser — `parser/types.rs:213-219`.
- Resolver — `resolve/mod.rs:2492-2499`.
- Layout: fat pointer, 16 bytes, align 8 — `codegen/layout.rs:333-335`.
- Test for layout — `codegen/tests.rs:270`.
- **No vtable.** No codegen for method calls through `dyn`. No
  object-safety check. No construction syntax that actually produces
  a fat pointer.

### 2.2 No fixture uses `dyn Trait`

The tutorial mentions it (`docs/tutorial/08-traits.md:98-108`) with a
note about the difference from `impl Trait`, but no fixture exercises
it. Today, typing `fn f(x: &dyn Displayable)` probably compiles but
does not produce a real trait-object argument — the resolver attaches
`Ty::DynTrait(...)`, the layout says 16 bytes, but method dispatch
falls back to structural matching via `TraitResolver::lookup_method`
which has no dyn-specific path.

### 2.3 Trait resolution distinguishes nominal vs structural

`typeck/traits.rs:83-133` splits on `require_nominal`:

```rust
if require_nominal {
    return TraitSatisfaction::Unsatisfied { ... };
}
```

The idea is right: `dyn Trait` requires nominal satisfaction (an
explicit `impl Trait for T`), while `impl Trait` accepts structural.
But the check is not wired up to the resolver's decision to use
`DynTrait` vs `ImplTrait` at type expressions, and no vtable is emitted
either way.

### 2.4 Drop-as-method infrastructure exists in spec

Tier-1 doc 04 §4 specifies `trait Drop` with `def mut drop`. Drop's
vtable slot is the "first method, always emitted" in most runtimes.
This doc references tier-1 doc 04; implementation must come after
tier-1 doc 04 ships (else `dyn Trait` leaks when dropped).

## 3. Goals & Non-goals

### Goals

- **G1.** `dyn Trait` is a fat pointer: `(data_ptr, vtable_ptr)`.
  Layout already asserts this.
- **G2.** Method dispatch through `dyn` uses the vtable. Indirect
  call, no inlining.
- **G3.** `Box[dyn Trait]` is the owning form. Dropping a
  `Box[dyn Trait]` calls the vtable's drop slot, then frees the box.
- **G4.** `&dyn Trait` and `&mut dyn Trait` are the borrowing forms.
- **G5.** Object-safety is checked at the type-declaration site.
  A non-object-safe trait cannot be used as `dyn Trait`.
- **G6.** Structural satisfaction is **not** accepted for
  `dyn Trait`. Only types with `impl Trait for T` can be coerced into
  `dyn Trait`. Matches Rust.
- **G7.** Vtable layout is stable and specified.
- **G8.** Unsized coercion: `&T → &dyn Trait`, `Box[T] → Box[dyn
  Trait]`, produced automatically at appropriate type boundaries.

### Non-goals

- **NG1.** Multi-trait objects (`dyn A + B`). Rust supports only
  `dyn A + auto_trait` (Send, Sync). Without auto-traits, multi-
  trait objects require combined vtables, which is a rabbit hole.
  Accept the tier-1 `auto Send + Sync` case only; reject
  `dyn Displayable + Comparable` with E-DYN-MULTI.
- **NG2.** Downcasting (`Any` trait). Separate feature.
- **NG3.** `dyn Trait` by-value on the stack (DST / unsized locals).
  Rust doesn't allow this without `Box`/`&`. Riven follows.
- **NG4.** Custom vtable layouts (`repr(C, dyn)` or similar). Fixed
  layout specified in §5.
- **NG5.** Trait upcasting (`&dyn Sub` → `&dyn Super`). Rust shipped
  this in 2024; separate phase 06c.

## 4. Object-Safety Rules

A trait is *object-safe* iff **every** method in the trait (including
inherited methods) satisfies:

### 4.1 Methods

- **S1.** Does not use `Self` in the return type *by value*. May use
  `&Self`, `&mut Self`, `Box[Self]`, `&dyn Trait` to self.
- **S2.** Does not use `Self` in an argument type *by value*. Same
  rule — references and box-wrapping are OK.
- **S3.** Does not have additional generic type parameters
  (`def foo[T](&self, x: T)`). The vtable would need one slot per
  monomorphization, which is infinite. Generic lifetimes are fine.
- **S4.** Is not a class method (`def self.new`). No receiver → can't
  dispatch.
- **S5.** Does not have `consume self` as its receiver (self by-value).
  The vtable has a single function pointer; a by-value self can't be
  passed across the abstract boundary without monomorphization.
  (Rust has `where Self: Sized` exemption; we omit for simplicity —
  if you want `consume self`, don't put it in a `dyn` trait.)

### 4.2 Associated items

- **S6.** Associated types are permitted only if the use-site binds
  them: `dyn Iterator[Item = Int]` is object-safe; `dyn Iterator` is
  not (because the vtable can't predict the item type). Every
  associated type in the trait must be equality-constrained at the
  `dyn` use site.
- **S7.** Generic associated types (GATs, doc 05) may never be bound
  at a `dyn` use site (the binding would be type-level-dependent —
  vtable can't express "function from lifetime to type"). Trait with
  a GAT → not object-safe. E-DYN-GAT.
- **S8.** Associated constants (not in tier 2) would require a vtable
  slot. Moot for now.

### 4.3 Trait supertrait bounds

- **S9.** Every supertrait must itself be object-safe, *and* its
  associated types must be bound at the `dyn` site. Recursive.

### 4.4 Diagnostic

The check runs at both:

- **Trait declaration time** — pre-compute `TraitInfo.object_safe:
  bool` once per trait. Cheap to consult.
- **`dyn Trait[Item = X]` use-site** — if the trait is object-safe
  only with bound associated types, verify the user provided them.

Errors:

- `E-DYN-NOT-SAFE { trait, reason }` — reason names the violating
  method or item.
- `E-DYN-ASSOC-UNBOUND { trait, name }` — missing associated-type
  binding at `dyn` site.
- `E-DYN-GAT` — trait has a GAT, therefore not object-safe.
- `E-DYN-MULTI` — multi-trait object (NG1).

## 5. Vtable Layout

### 5.1 Layout

Each `dyn Trait` produces exactly one vtable per concrete (type, trait)
pair. The vtable is a read-only global:

```
struct VTable_<Trait>_<Type> {
    drop: fn(*mut u8),                  // slot 0: drop glue
    size: usize,                         // slot 1: byte size of T
    align: usize,                        // slot 2: byte alignment of T
    method_0: fn(...args) -> ret,        // slot 3: trait methods
    method_1: fn(...args) -> ret,
    ...
};
```

Slot 0, 1, 2 are fixed. Slots 3.. are in **trait-declaration order**
(not alphabetical — matches Rust's layout).

Rationale for 0/1/2:

- `drop` (slot 0): called when a `Box[dyn Trait]` is dropped. Points
  to the type's drop glue (tier-1 doc 04 §7).
- `size` (slot 1): needed for `Box[dyn Trait]`'s heap free — the
  allocator needs to know how many bytes to free. Same for
  `riven_dealloc`.
- `align` (slot 2): needed for allocation.

### 5.2 Fat-pointer representation

`dyn Trait` at runtime is:

```c
struct FatPtr {
    void *data;          // pointer to the concrete value
    const VTable *vtbl;
};
```

This matches the 16-byte / 8-align layout at `codegen/layout.rs:334`.

### 5.3 Method-call lowering

`x.method(args)` where `x: &dyn Trait`:

```
load vtbl = x.vtbl
load fn = vtbl.method_N       // N = method's index in the trait
call fn(x.data, args...)
```

The receiver passed to the vtable's function is the *data pointer*,
typed `*mut u8` at the ABI boundary. The concrete function
monomorphization receives it cast to `&mut ConcreteType` via the
shim. See §5.5 for ABI detail.

### 5.4 Drop lowering

`drop(box_val)` where `box_val: Box[dyn Trait]`:

```
load vtbl = box_val.vtbl
load drop_fn = vtbl.drop
load size = vtbl.size
call drop_fn(box_val.data)
call riven_dealloc(box_val.data, size)  // or riven_dealloc(box_val.data) if the runtime tracks sizes
```

The heap-free variant depends on the runtime. Today `riven_dealloc`
takes a pointer and calls `free` (`runtime/runtime.c:156-158`); the
vtable's `size` field is informational but may enable a future
size-classed allocator.

### 5.5 ABI shim

Concrete methods take `&Concrete` (a typed pointer); the vtable slot
holds a `fn(*mut u8, args...) -> ret`. The compiler emits a
per-(type, method) *ABI shim*:

```c
/* Generated shim for Square::area: */
int64_t __shim_Square_area(void *self_data, ...) {
    Square *s = (Square *)self_data;
    return Square_area(s);
}
```

The shim is trivial (a cast + tail call). It preserves the
`def mut foo(&mut self, ...)` self-mode correctly because Riven's
calling convention is the same whether self is typed as `*mut u8` or
`&mut Concrete` at the ABI level.

For closure types (`Fn`, `FnMut`, `FnOnce`), the shim points to the
closure's invoke function (which already exists for each closure).

### 5.6 Vtable emission

One vtable per (concrete type, trait) pair used in the program. M2
monomorphization has the full list of `dyn Trait` coercions; it walks
them and emits one vtable each. Deduplication: if `Square: Displayable`
is coerced to `dyn Displayable` from both `&Square` and
`Box[Square]`, only one vtable is emitted.

Vtables live in read-only data (`.rodata`).

## 6. Surface Syntax

### 6.1 Declaration

No new syntax. `dyn Trait` already parses.

### 6.2 Construction

```riven
class Square; side: Float; ...; end
impl Displayable for Square; def to_display -> String { "Square" }; end

let s = Square.new(1.0)
let d: &dyn Displayable = &s            # coercion
let boxed: Box[dyn Displayable] = Box.new(s)  # unsized coercion
```

Rules:

- `&T → &dyn Trait` and `&mut T → &mut dyn Trait` coerce
  automatically where the target type is `&dyn Trait` and `T: Trait`
  nominally. The coercion is implicit, at assignment and at function
  call.
- `Box[T] → Box[dyn Trait]` same.
- No other types may be coerced (e.g., `Vec[T] → Vec[dyn Trait]` is
  an E-DYN-NO-DEEP-COERCE; the user writes `vec.map(|x| Box.new(x))`).
- `as dyn Trait` explicit coercion is legal and documented.

### 6.3 Use

```riven
def print_all(items: &Vec[Box[dyn Displayable]])
  for item in items
    puts item.to_display
  end
end
```

### 6.4 Auto-derive `DynSafe`

Reserved for tier-2: an attribute `@[derive(DynSafe)]` or a compile-
time query `T.is_dyn_safe`. Not in scope for phase 06a. Design hook:
the object-safety check already computes a per-trait
`object_safe: bool`; exposing it as a user-visible predicate is a
one-liner.

## 7. Implementation Plan

### 7.1 Code map

| Change | File(s) |
|---|---|
| `TraitInfo.object_safe: bool` | `resolve/symbols.rs:60-66` |
| Object-safety pass | new `typeck/object_safety.rs` |
| Use-site check for bound assoc types | `typeck/coerce.rs` + unify |
| Coercion `&T → &dyn Trait` | `typeck/coerce.rs` |
| Vtable emission | new `codegen/vtable.rs` |
| ABI shim emission | `codegen/cranelift.rs`, `codegen/llvm/emit.rs` |
| Method dispatch through vtable | `codegen/cranelift.rs` (MethodCall arm) |
| Drop through vtable | `codegen/cranelift.rs` MirInst::Drop arm |
| Box[T] coercion to Box[dyn Trait] | `codegen/cranelift.rs` |
| Error codes | `diagnostics/` |

### 7.2 Phasing

**Phase 06a — object safety + vtable emission (2 weeks).**

1. Implement rules S1-S9.
2. Compute `TraitInfo.object_safe` after trait registration.
3. Reject `Ty::DynTrait` use of non-safe traits.
4. Vtable layout and emission for `(type, trait)` pairs.
5. Method dispatch through vtable.
6. Drop through vtable (depends on tier-1 doc 04 phase 4a).

At the end of 06a, `&dyn Displayable` works end-to-end; a fixture
with `Vec[Box[dyn Displayable]]` compiles and runs.

**Phase 06b — ergonomic coercions (1 week).**

7. Implicit `&T → &dyn Trait` at assignment and call boundaries.
8. Implicit `Box[T] → Box[dyn Trait]` same.
9. Explicit `as dyn Trait` syntax.
10. Diagnostic suggestions: "you may have meant &dyn Trait".

**Phase 06c — later (optional).**

11. Trait upcasting: `&dyn Sub → &dyn Super`.
12. `@[derive(DynSafe)]` + compile-time query.
13. Multi-trait objects (rejected as NG1 in tier 2).

## 8. Interactions With Other Tier-2 Features

### 8.1 With associated types (doc 01)

Trait with associated type A is object-safe *iff* A is bound at the
`dyn` use site. `dyn Iterator[Item = Int]` works; `dyn Iterator` does
not. Rule S6 in §4. Documented E-DYN-ASSOC-UNBOUND.

### 8.2 With GATs (doc 05)

Trait with a GAT is never object-safe. Rule S7.
E-DYN-GAT.

### 8.3 With HRTBs (doc 03)

`dyn for['a] Fn(&'a T)` is object-safe iff `Fn` is. The HRTB affects
only the *type* of the trait object; the vtable is identical.

### 8.4 With const generics (doc 02)

`dyn FixedBuffer[4]` is object-safe; the const must be bound at the
use site (same as associated types).

### 8.5 With variance (doc 07)

`Ty::DynTrait(bounds)` is invariant in its bounds — there is no
general subtyping between `dyn A` and `dyn B`. Trait upcasting
(06c) adds a limited form; see doc 07 §5.

### 8.6 With impl Trait (doc 04)

Distinct feature: `impl Trait` is static dispatch, `dyn Trait` is
dynamic. User-visible choice. No shared code path.

### 8.7 With tier-1 Drop (doc 04)

Vtable's drop slot is the drop glue from tier-1 §7. Object-safe
traits must be droppable. Tier-1 is a hard prerequisite.

## 9. Open Questions & Risks

- **OQ-1: consume self on trait objects.** Rule S5 forbids it.
  Exception: Rust has `where Self: Sized` to opt out per method —
  the method is not in the vtable, but is callable on the concrete
  type. Useful for `trait IntoIterator; fn into_iter(self) -> Self.Iter`.
  Recommendation: add in 06a or explicitly defer. See tutorial's
  `docs/tutorial/08-traits.md:108`.
- **OQ-2: vtable alignment.** 8-byte on 64-bit. ARM32 / wasm32 would
  need 4-byte. Riven currently targets 64-bit only (tier-1 assumes
  `ISize/USize == 64`).
- **OQ-3: vtable equality.** Two vtables for `(Square, Displayable)`
  emitted in different compilation units — must they be pointer-
  equal? Not strictly needed for correctness, but a convenience for
  code that compares trait objects. Deduplicate at link time if
  possible.
- **OQ-4: Box vs Rc vs Arc for dyn.** `Rc[dyn Trait]` and
  `Arc[dyn Trait]` are analogous to `Box[dyn Trait]`. Rc/Arc are
  stdlib features (tier-1 doc 02 concurrency) — ensure they
  smart-pointer the fat pointer through their container.
- **R-1: ABI stability.** Once vtable layout is shipped, users can
  cast function pointers in unsafe code. Stabilising vtable layout
  constrains future flexibility. Document as not-guaranteed for
  external ABI use.
- **R-2: stack usage for large dyn bodies.** A trait method that
  takes `&mut self` on a 10MB struct is called with a fat pointer
  and operates through indirection — no stack issue. But a
  `consume self` method on a small struct was rejected (S5); users
  who want move-by-value must refactor.
- **R-3: performance of indirect calls.** Modern CPUs predict them
  well but not perfectly. `dyn Trait` is the right choice for
  heterogeneous collections, plugin boundaries, and callbacks —
  *not* for hot inner loops. Document.
- **R-4: layout drift.** Before 06a, `Ty::DynTrait` layouts to 16
  bytes and is a placeholder. After 06a, it's a real fat pointer
  with specific layout. Any FFI code that happens to pass a
  `dyn Trait` will need recompilation. Expect no user breakage
  today because no FFI path uses dyn (`runtime/runtime.c` has none).

## 10. Test Matrix

### 10.1 Positive tests

- T1: `fn show(x: &dyn Displayable)` called with `&Square.new(...)`.
  Displays correctly.
- T2: `Vec[Box[dyn Displayable]]` holding three different shape
  types. Iteration produces the right strings.
- T3: Drop on Box[dyn Trait]: a type with a user Drop is dropped
  once when the box drops.
- T4: Method with `&mut self` through vtable: `fn increment(c: &mut
  dyn Counter)` increments a concrete counter type.
- T5: `dyn Iterator[Item = Int]` — bound associated type.
- T6: Implicit coercion: `let boxed: Box[dyn Displayable] =
  Box.new(Square.new(1.0))`.

### 10.2 Negative tests

- N1: Generic method on trait: `trait T; def foo[U](self, x: U)
  end`. Use `dyn T` → E-DYN-NOT-SAFE (S3).
- N2: `dyn Iterator` without bound → E-DYN-ASSOC-UNBOUND (S6).
- N3: `dyn LendingIterator` (GAT) → E-DYN-GAT (S7).
- N4: Class method in trait: `def self.new ...` → E-DYN-NOT-SAFE (S4).
- N5: `consume self` method in trait → E-DYN-NOT-SAFE (S5).
- N6: Multi-trait object `dyn A + B` → E-DYN-MULTI (NG1).
- N7: Struct field of unsized `dyn Trait` (by value) →
  E-DYN-UNSIZED-FIELD.
- N8: Structural satisfaction only (no `impl T for Square` block) →
  E-DYN-NO-IMPL.

### 10.3 Fixture additions

- `tests/fixtures/dyn_basic.rvn` — shape hierarchy with
  `Vec[Box[dyn Drawable]]`.
- `tests/fixtures/dyn_iterator.rvn` — `dyn Iterator[Item = Int]`.
- `tests/fixtures/dyn_callback.rvn` — `Box[dyn Fn()]` field.
- `tests/fixtures/dyn_error_not_safe.rvn` — negative: generic
  method.
