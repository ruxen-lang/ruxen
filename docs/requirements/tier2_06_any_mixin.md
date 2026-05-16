# Tier 2.06 — Mixin Objects (`any Mixin`)

Status: Draft (requirements)
Owner: compiler
Depends on: tier-1 doc 04 (Drop) for vtable drop slot; associated types
            (doc 01) for projection-safety rule
Blocks: plugin/script interfaces; heterogeneous collections
        (`Array[Box[any Drawable]]`); event-driven code

## 1. Summary & Motivation

A *mixin object* erases the concrete type of a value behind a mixin:
`any Mixin` is a runtime-dispatched view through which you can call
the mixin's methods without knowing the underlying type. The canonical
use cases:

1. **Heterogeneous collections.** `Array[Box[any Drawable]]` holds
   `Square`, `Circle`, `Triangle` in the same vector.
2. **Plugin boundaries.** Loaded-at-runtime modules expose a
   `any Command` interface; the runtime doesn't know the concrete
   type.
3. **Callbacks stored in fields.** `on_click: Box[any Fn()]`.
4. **Type-erased error types.** `Box[any Error]` — a common stdlib
   pattern.

The compiler already has most of the type-level scaffolding: `Ty::DynTrait(Vec<TraitRef>)`
(`hir/types.rs:115`), layout as 16-byte fat pointer
(`codegen/layout.rs:333-335`). What's missing: vtable emission,
object-safety checking, method-call lowering to vtable dispatch,
drop-via-vtable. This doc specifies the complete set of changes to
ship `any Mixin` end-to-end.

## 2. Current State

### 2.1 Type, parse, layout all exist; nothing else

- `Ty::DynTrait(Vec<TraitRef>)` — `hir/types.rs:112-115`.
- Parser — `parser/types.rs:213-219`.
- Resolver — `resolve/mod.rs:2492-2499`.
- Layout: fat pointer, 16 bytes, align 8 — `codegen/layout.rs:333-335`.
- Test for layout — `codegen/tests.rs:270`.
- **No vtable.** No codegen for method calls through `any`. No
  object-safety check. No construction syntax that actually produces
  a fat pointer.

### 2.2 No fixture uses `any Mixin`

The tutorial mentions it (`docs/tutorial/08-mixins.md:98-108`) with a
note about the difference from `some Mixin`, but no fixture exercises
it. Today, typing `fn f(x: &any Display)` probably compiles but
does not produce a real mixin-object argument — the resolver attaches
`Ty::DynTrait(...)`, the layout says 16 bytes, but method dispatch
falls back to structural matching via `TraitResolver::lookup_method`
which has no any-specific path.

### 2.3 Mixin resolution distinguishes nominal vs structural

`typeck/traits.rs:83-133` splits on `require_nominal`:

```rust
if require_nominal {
    return TraitSatisfaction::Unsatisfied { ... };
}
```

The idea is right: `any Mixin` requires nominal satisfaction (an
explicit `include Mixin` directive in `T`'s body), while `some Mixin` accepts structural.
But the check is not wired up to the resolver's decision to use
`DynTrait` vs `ImplTrait` at type expressions, and no vtable is emitted
either way.

### 2.4 Drop-as-method infrastructure exists in spec

Tier-1 doc 04 §4 specifies `mixin Drop` with `def var drop`. Drop's
vtable slot is the "first method, always emitted" in most runtimes.
This doc references tier-1 doc 04; implementation must come after
tier-1 doc 04 ships (else `any Mixin` leaks when dropped).

## 3. Goals & Non-goals

### Goals

- **G1.** `any Mixin` is a fat pointer: `(data_ptr, vtable_ptr)`.
  Layout already asserts this.
- **G2.** Method dispatch through `any` uses the vtable. Indirect
  call, no inlining.
- **G3.** `Box[any Mixin]` is the owning form. Dropping a
  `Box[any Mixin]` calls the vtable's drop slot, then frees the box.
- **G4.** `&any Mixin` is the borrowing form. (Per the canonical spec §3.4,
  `&var any Mixin` does not exist — mutating through an existential requires
  taking ownership via `Box[any Mixin]`, `Shared[any Mixin]`, or
  `SharedSync[any Mixin]`.)
- **G5.** Object-safety is checked at the type-declaration site.
  A non-object-safe mixin cannot be used as `any Mixin`.
- **G6.** Structural satisfaction is **not** accepted for
  `any Mixin`. Only types with an explicit `include Mixin` directive in
  their body can be coerced into `any Mixin`. Matches Rust.
- **G7.** Vtable layout is stable and specified.
- **G8.** Unsized coercion: `&T → &any Mixin`, `Box[T] → Box[any
  Mixin]`, produced automatically at appropriate type boundaries.

### Non-goals

- **NG1.** Multi-mixin objects (`any A + B`). Rust supports only
  `any A + auto_trait` (Send, Sync). Without auto-traits, multi-
  mixin objects require combined vtables, which is a rabbit hole.
  Accept the tier-1 `auto Send + Sync` case only; reject
  `any Display + Ord` with E-DYN-MULTI.
- **NG2.** Downcasting (`Any` mixin). Separate feature.
- **NG3.** `any Mixin` by-value on the stack (DST / unsized locals).
  Rust doesn't allow this without `Box`/`&`. Riven follows.
- **NG4.** Custom vtable layouts (`repr(C, any)` or similar). Fixed
  layout specified in §5.
- **NG5.** Mixin upcasting (`&any Sub` → `&any Super`). Rust shipped
  this in 2024; separate phase 06c.

## 4. Object-Safety Rules

A mixin is *object-safe* iff **every** method in the mixin (including
inherited methods) satisfies:

### 4.1 Methods

- **S1.** Does not use `Self` in the return type *by value*. May use
  `&Self`, `&var Self`, `Box[Self]`, `&any Mixin` to self.
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
  if you want `consume self`, don't put it in a `any` mixin.)

### 4.2 Associated items

- **S6.** Associated types are permitted only if the use-site binds
  them: `any Iterator[Item = Int]` is object-safe; `any Iterator` is
  not (because the vtable can't predict the item type). Every
  associated type in the mixin must be equality-constrained at the
  `any` use site.
- **S7.** Generic associated types (GATs, doc 05) may never be bound
  at a `any` use site (the binding would be type-level-dependent —
  vtable can't express "function from lifetime to type"). Mixin with
  a GAT → not object-safe. E-DYN-GAT.
- **S8.** Associated constants (not in tier 2) would require a vtable
  slot. Moot for now.

### 4.3 Mixin supertrait bounds

- **S9.** Every supertrait must itself be object-safe, *and* its
  associated types must be bound at the `any` site. Recursive.

### 4.4 Diagnostic

The check runs at both:

- **Mixin declaration time** — pre-compute `TraitInfo.object_safe:
  bool` once per mixin. Cheap to consult.
- **`any Mixin[Item = X]` use-site** — if the mixin is object-safe
  only with bound associated types, verify the user provided them.

Errors:

- `E-DYN-NOT-SAFE { mixin, reason }` — reason names the violating
  method or item.
- `E-DYN-ASSOC-UNBOUND { mixin, name }` — missing associated-type
  binding at `any` site.
- `E-DYN-GAT` — mixin has a GAT, therefore not object-safe.
- `E-DYN-MULTI` — multi-mixin object (NG1).

## 5. Vtable Layout

### 5.1 Layout

Each `any Mixin` produces exactly one vtable per concrete (type, mixin)
pair. The vtable is a read-only global:

```
struct VTable_<Mixin>_<Type> {
    drop: fn(*var u8),                  // slot 0: drop glue
    size: usize,                         // slot 1: byte size of T
    align: usize,                        // slot 2: byte alignment of T
    method_0: fn(...args) -> ret,        // slot 3: mixin methods
    method_1: fn(...args) -> ret,
    ...
};
```

Slot 0, 1, 2 are fixed. Slots 3.. are in **mixin-declaration order**
(not alphabetical — matches Rust's layout).

Rationale for 0/1/2:

- `drop` (slot 0): called when a `Box[any Mixin]` is dropped. Points
  to the type's drop glue (tier-1 doc 04 §7).
- `size` (slot 1): needed for `Box[any Mixin]`'s heap free — the
  allocator needs to know how many bytes to free. Same for
  `riven_dealloc`.
- `align` (slot 2): needed for allocation.

### 5.2 Fat-pointer representation

`any Mixin` at runtime is:

```c
struct FatPtr {
    void *data;          // pointer to the concrete value
    const VTable *vtbl;
};
```

This matches the 16-byte / 8-align layout at `codegen/layout.rs:334`.

### 5.3 Method-call lowering

`x.method(args)` where `x: &any Mixin`:

```
load vtbl = x.vtbl
load fn = vtbl.method_N       // N = method's index in the mixin
call fn(x.data, args...)
```

The receiver passed to the vtable's function is the *data pointer*,
typed `*var u8` at the ABI boundary. The concrete function
monomorphization receives it cast to `&var ConcreteType` via the
shim. See §5.5 for ABI detail.

### 5.4 Drop lowering

`drop(box_val)` where `box_val: Box[any Mixin]`:

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
holds a `fn(*var u8, args...) -> ret`. The compiler emits a
per-(type, method) *ABI shim*:

```c
/* Generated shim for Square::area: */
int64_t __shim_Square_area(void *self_data, ...) {
    Square *s = (Square *)self_data;
    return Square_area(s);
}
```

The shim is trivial (a cast + tail call). It preserves the
`def var foo(&var self, ...)` self-mode correctly because Riven's
calling convention is the same whether self is typed as `*var u8` or
`&var Concrete` at the ABI level.

For closure types (`Fn`, `FnVar`, `FnOnce`), the shim points to the
closure's invoke function (which already exists for each closure).

### 5.6 Vtable emission

One vtable per (concrete type, mixin) pair used in the program. M2
monomorphization has the full list of `any Mixin` coercions; it walks
them and emits one vtable each. Deduplication: if `Square: Display`
is coerced to `any Display` from both `&Square` and
`Box[Square]`, only one vtable is emitted.

Vtables live in read-only data (`.rodata`).

## 6. Surface Syntax

### 6.1 Declaration

No new syntax. `any Mixin` already parses.

### 6.2 Construction

```riven
class Square
  side: Float
  include Display
  def to_display -> String; "Square"; end
end

let s = Square.new(1.0)
let d: &any Display = &s            # coercion
let boxed: Box[any Display] = Box.new(s)  # unsized coercion
```

Rules:

- `&T → &any Mixin` coerces automatically where the target type
  is `&any Mixin` and `T: Mixin` nominally. The coercion is implicit,
  at assignment and at function call. (Per spec §3.4, `&var any Mixin`
  does not exist — mutable existential access requires `Box[any Mixin]`
  or one of the shared owning forms.)
- `Box[T] → Box[any Mixin]` same.
- No other types may be coerced (e.g., `Array[T] → Array[any Mixin]` is
  an E-DYN-NO-DEEP-COERCE; the user writes `vec.map(|x| Box.new(x))`).
- `as any Mixin` explicit coercion is legal and documented.

### 6.3 Use

```riven
def print_all(items: &Array[Box[any Display]])
  for item in items
    puts item.to_display
  end
end
```

### 6.4 Auto-derive `DynSafe`

Reserved for tier-2: an in-body `derive(DynSafe)` directive or a compile-
time query `T.is_dyn_safe`. Not in scope for phase 06a. Design hook:
the object-safety check already computes a per-mixin
`object_safe: bool`; exposing it as a user-visible predicate is a
one-liner.

## 7. Implementation Plan

### 7.1 Code map

| Change | File(s) |
|---|---|
| `TraitInfo.object_safe: bool` | `resolve/symbols.rs:60-66` |
| Object-safety pass | new `typeck/object_safety.rs` |
| Use-site check for bound assoc types | `typeck/coerce.rs` + unify |
| Coercion `&T → &any Mixin` | `typeck/coerce.rs` |
| Vtable emission | new `codegen/vtable.rs` |
| ABI shim emission | `codegen/cranelift.rs`, `codegen/llvm/emit.rs` |
| Method dispatch through vtable | `codegen/cranelift.rs` (MethodCall arm) |
| Drop through vtable | `codegen/cranelift.rs` MirInst::Drop arm |
| Box[T] coercion to Box[any Mixin] | `codegen/cranelift.rs` |
| Error codes | `diagnostics/` |

### 7.2 Phasing

**Phase 06a — object safety + vtable emission (2 weeks).**

1. Implement rules S1-S9.
2. Compute `TraitInfo.object_safe` after mixin registration.
3. Reject `Ty::DynTrait` use of non-safe traits.
4. Vtable layout and emission for `(type, mixin)` pairs.
5. Method dispatch through vtable.
6. Drop through vtable (depends on tier-1 doc 04 phase 4a).

At the end of 06a, `&any Display` works end-to-end; a fixture
with `Array[Box[any Display]]` compiles and runs.

**Phase 06b — ergonomic coercions (1 week).**

7. Implicit `&T → &any Mixin` at assignment and call boundaries.
8. Implicit `Box[T] → Box[any Mixin]` same.
9. Explicit `as any Mixin` syntax.
10. Diagnostic suggestions: "you may have meant &any Mixin".

**Phase 06c — later (optional).**

11. Mixin upcasting: `&any Sub → &any Super`.
12. In-body `derive(DynSafe)` directive + compile-time query.
13. Multi-mixin objects (rejected as NG1 in tier 2).

## 8. Interactions With Other Tier-2 Features

### 8.1 With associated types (doc 01)

Mixin with associated type A is object-safe *iff* A is bound at the
`any` use site. `any Iterator[Item = Int]` works; `any Iterator` does
not. Rule S6 in §4. Documented E-DYN-ASSOC-UNBOUND.

### 8.2 With GATs (doc 05)

Mixin with a GAT is never object-safe. Rule S7.
E-DYN-GAT.

### 8.3 With HRTBs (doc 03)

`any for['a] Fn(&'a T)` is object-safe iff `Fn` is. The HRTB affects
only the *type* of the mixin object; the vtable is identical.

### 8.4 With const generics (doc 02)

`any FixedBuffer[4]` is object-safe; the const must be bound at the
use site (same as associated types).

### 8.5 With variance (doc 07)

`Ty::DynTrait(bounds)` is invariant in its bounds — there is no
general subtyping between `any A` and `any B`. Mixin upcasting
(06c) adds a limited form; see doc 07 §5.

### 8.6 With some Mixin (doc 04)

Distinct feature: `some Mixin` is static dispatch, `any Mixin` is
dynamic. User-visible choice. No shared code path.

### 8.7 With tier-1 Drop (doc 04)

Vtable's drop slot is the drop glue from tier-1 §7. Object-safe
traits must be droppable. Tier-1 is a hard prerequisite.

## 9. Open Questions & Risks

- **OQ-1: consume self on mixin objects.** Rule S5 forbids it.
  Exception: Rust has `where Self: Sized` to opt out per method —
  the method is not in the vtable, but is callable on the concrete
  type. Useful for `mixin IntoIterator; fn into_iter(self) -> Self.Iter`.
  Recommendation: add in 06a or explicitly defer. See tutorial's
  `docs/tutorial/08-mixins.md:108`.
- **OQ-2: vtable alignment.** 8-byte on 64-bit. ARM32 / wasm32 would
  need 4-byte. Riven currently targets 64-bit only (tier-1 assumes
  `ISize/USize == 64`).
- **OQ-3: vtable equality.** Two vtables for `(Square, Display)`
  emitted in different compilation units — must they be pointer-
  equal? Not strictly needed for correctness, but a convenience for
  code that compares mixin objects. Deduplicate at link time if
  possible.
- **OQ-4: Box vs Rc vs Arc for any.** `Shared[any Mixin]` and
  `SharedSync[any Mixin]` are analogous to `Box[any Mixin]`. Rc/Arc are
  stdlib features (tier-1 doc 02 concurrency) — ensure they
  smart-pointer the fat pointer through their container.
- **R-1: ABI stability.** Once vtable layout is shipped, users can
  cast function pointers in unsafe code. Stabilising vtable layout
  constrains future flexibility. Document as not-guaranteed for
  external ABI use.
- **R-2: stack usage for large any bodies.** A mixin method that
  takes `&var self` on a 10MB struct is called with a fat pointer
  and operates through indirection — no stack issue. But a
  `consume self` method on a small struct was rejected (S5); users
  who want move-by-value must refactor.
- **R-3: performance of indirect calls.** Modern CPUs predict them
  well but not perfectly. `any Mixin` is the right choice for
  heterogeneous collections, plugin boundaries, and callbacks —
  *not* for hot inner loops. Document.
- **R-4: layout drift.** Before 06a, `Ty::DynTrait` layouts to 16
  bytes and is a placeholder. After 06a, it's a real fat pointer
  with specific layout. Any FFI code that happens to pass a
  `any Mixin` will need recompilation. Expect no user breakage
  today because no FFI path uses any (`runtime/runtime.c` has none).

## 10. Test Matrix

### 10.1 Positive tests

- T1: `fn show(x: &any Display)` called with `&Square.new(...)`.
  Displays correctly.
- T2: `Array[Box[any Display]]` holding three different shape
  types. Iteration produces the right strings.
- T3: Drop on Box[any Mixin]: a type with a user Drop is dropped
  once when the box drops.
- T4: Method with `&var self` through vtable: `fn increment(c: &var
  any Counter)` increments a concrete counter type.
- T5: `any Iterator[Item = Int]` — bound associated type.
- T6: Implicit coercion: `let boxed: Box[any Display] =
  Box.new(Square.new(1.0))`.

### 10.2 Negative tests

- N1: Generic method on mixin: `mixin T; def foo[U](self, x: U)
  end`. Use `any T` → E-DYN-NOT-SAFE (S3).
- N2: `any Iterator` without bound → E-DYN-ASSOC-UNBOUND (S6).
- N3: `any LendingIterator` (GAT) → E-DYN-GAT (S7).
- N4: Class method in mixin: `def self.new ...` → E-DYN-NOT-SAFE (S4).
- N5: `consume self` method in mixin → E-DYN-NOT-SAFE (S5).
- N6: Multi-mixin object `any A + B` → E-DYN-MULTI (NG1).
- N7: Struct field of unsized `any Mixin` (by value) →
  E-DYN-UNSIZED-FIELD.
- N8: Structural satisfaction only (no `include T` directive in `Square`'s body) →
  E-DYN-NO-IMPL.

### 10.3 Fixture additions

- `tests/fixtures/dyn_basic.rvn` — shape hierarchy with
  `Array[Box[any Drawable]]`.
- `tests/fixtures/dyn_iterator.rvn` — `any Iterator[Item = Int]`.
- `tests/fixtures/dyn_callback.rvn` — `Box[any Fn()]` field.
- `tests/fixtures/dyn_error_not_safe.rvn` — negative: generic
  method.
