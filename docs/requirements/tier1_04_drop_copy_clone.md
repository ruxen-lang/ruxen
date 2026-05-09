# Tier 1 — Drop, Copy, Clone

Status: Draft (requirements)
Owner: compiler
Depends on: attribute parsing (existing), trait resolution (existing),
            MIR drop insertion stub (existing), panic strategy (follow-up)

## 1. Summary & Motivation

Riven promises Rust-style deterministic ownership: "when the owner goes out
of scope, the value is dropped (destructor runs, memory freed)"
(`docs/tutorial/04-ownership-and-borrowing.md:11`). Today that promise is
**not kept at the code-generation level** — locals of heap-allocated types
are marked with `MirInst::Drop` in MIR, but both backends turn `Drop` into
a no-op (`crates/riven-core/src/codegen/cranelift.rs:692-698`,
`crates/riven-core/src/codegen/llvm/emit.rs:790-792`), so every Riven
program leaks memory until `exit(3)` reclaims it.

This document specifies the `Drop`, `Copy` and `Clone` traits, the
drop-insertion algorithm, derive support, and the borrow-check/codegen
changes required to fulfil rule 5. It covers the three traits as a single
Tier-1 feature because they are deeply interdependent:

- `Drop` needs move-tracking to avoid double-free.
- `Copy` must forbid `Drop` (otherwise copying a resource-holding type
  would double-free).
- `Clone` is the explicit escape hatch when `Copy` does not apply; `Copy`
  types must always be `Clone`.

**Motivating use cases** (all currently unexpressible or leaky):

- File/socket handles that close on scope exit.
- Mutex/lock guards (`MutexGuard`-style).
- Arena/region allocators that free on drop.
- String / Vec buffers freed instead of leaked.
- Reference-counted smart pointers (`Rc`), once we have them.

## 2. Current State

### 2.1 Types already know Copy vs. Move

`Ty::is_copy` in `crates/riven-core/src/hir/types.rs:189-221` hard-codes
the Copy set: all integer/float primitives, `Bool`, `Char`, `Unit`,
`Never`, `&T` / `&'a T` / `&str`, raw pointers, `Ty::Error`, plus tuples
and fixed arrays whose elements are Copy. Everything else (including
`String`, `Vec[T]`, `Hash[K,V]`, `Option[T]`, `Result[T,E]`, any user
`class`/`struct`/`enum`) reports `is_copy() == false`.

`MoveSemantics { Copy, Move }` is defined at
`crates/riven-core/src/hir/types.rs:178-182` and carried on every
`HirExprKind::Assign` (`hir/nodes.rs:148`). The type-checker fills it in
from `Ty::move_semantics()`; the borrow checker uses it in
`check_assign` (`borrow_check/mod.rs:440-487`).

There is currently **no user-visible way to change a type's move
semantics** — the `derive Copy` mentioned in the tutorial
(`docs/tutorial/04-ownership-and-borrowing.md:119-130`) is accepted by
the parser but never actually makes the struct Copy. The struct is still
Move because `Ty::is_copy` returns `false` for any `Ty::Struct { .. }`.

### 2.2 Built-in traits exist as names only

`Copy`, `Clone`, and `Drop` are registered as built-in traits with the
right method-name lists in `resolve/mod.rs:147-151`:

```rust
("Copy",  vec![]),
("Clone", vec!["clone"]),
("Drop",  vec!["drop"]),
```

Registration gives them `DefId`s and puts them in scope for
`impl Copy for T` or `@[derive(Copy)]`, but:

- `TraitResolver` (`typeck/traits.rs`) treats them as ordinary traits.
  There is no special-casing for `Copy` (marker with no methods), no
  mutual-exclusion check with `Drop`, no check that all fields satisfy
  the same bound.
- Implementing `Drop` for a user type has **no effect** on `is_copy`,
  on move semantics, or on codegen.
- `Ty::is_copy` ignores the trait table entirely.

### 2.3 MIR already inserts Drop (partially) — but codegen ignores it

`MirInst::Drop { local: LocalId }` exists at
`crates/riven-core/src/mir/nodes.rs:282-285`. `insert_drops` in
`crates/riven-core/src/mir/lower.rs:3346-3407` runs once per function
after lowering and appends Drop instructions **before every
`Terminator::Return`**, in reverse declaration order, for locals that:

- are Move types (`!is_copy()`),
- are not parameters,
- are not the return-value local,
- have a name that does not start with `_t` (skip compiler temps),
- are `Ty::Class { .. }` | `Ty::Struct { .. }` | `Ty::Enum { .. }` —
  strings, vecs, options, results, tuples, arrays are explicitly
  excluded (lower.rs:3379-3387) "because String/Vec/etc. may hold
  pointers to static data sections and can't be safely freed in v1".

Both backends then **treat Drop as a no-op**:

- Cranelift: `cranelift.rs:692-698` — comment says calling
  `riven_dealloc` here would double-free values moved into collections,
  because the drop pass does not track ownership transfers to callees.
- LLVM: `emit.rs:790-792` — same, "matches Cranelift backend".

Consequences:

- Owned user-defined heap objects (class/struct/enum) are leaked.
- `String` / `Vec` etc. are always leaked — they aren't even in the drop
  set.
- `Drop` is not a trait anyone can implement; user `def drop` methods on
  a class are never called.
- No drop on panic, break, continue, early return after move — but none
  of those paths call drop anyway.

### 2.4 Borrow checker tracks moves but not drops

`MoveChecker` (`borrow_check/moves.rs`) and `OwnershipState`
(`borrow_check/ownership.rs`) track per-`DefId` state (`Live | Moved |
PartiallyMoved | Uninitialized`). The checker conservatively merges
branch states ("moved on any branch → moved after"; moves.rs:100-114,
ownership.rs:95-124).

`check_method_call` (`borrow_check/mod.rs:605-743`) consumes the
receiver when `HirSelfMode::Consuming`, which is the mechanism a custom
`drop(consume self)` would eventually use. Name-based heuristics also
move the receiver for `into_iter` (mod.rs:726-735).

There is **no concept of drop flags** — i.e. the checker cannot tell the
MIR lowering pass "this local is conditionally moved; emit a runtime
flag and a conditional drop". Since drop insertion today is purely
syntactic (insert before every Return), conditionally-moved locals
would be double-freed the moment we turn Drop from a no-op into a real
call.

### 2.5 Runtime has allocation primitives but no typed free

`crates/riven-core/runtime/runtime.c:144-163` provides:

```c
void *riven_alloc(uint64_t size);         // malloc + zero + panic-on-null
void  riven_dealloc(void *ptr);           // free
void *riven_realloc(void *ptr, uint64_t);
```

`MirInst::Alloc` emits `riven_alloc(size)` (cranelift.rs:618-634). There
is no matching "typed dealloc" runtime helper and no per-type free
glue — the runtime assumes leaks are fine.

`String_clone` is mapped to `riven_string_from`
(`codegen/runtime.rs:59`), which does a real `malloc`+`memcpy`, so
string clone is already correct; other clone operations are not wired
up.

### 2.6 Attribute / derive syntax

Attributes are parsed as `@[name(args)]`
(`parser/mod.rs:1572-1610`). Today the attribute form is only applied to
`lib` and `struct` items (mod.rs:473-512), and for struct it only
understands `@[repr(...)]` (mod.rs:498-503).

A struct body may additionally contain a `derive Trait1, Trait2` line
(`parser/mod.rs:824-845`), which populates
`HirStructDef::derive_traits: Vec<String>` (`hir/nodes.rs:431`) and
`StructInfo::derive_traits` (`resolve/symbols.rs:48`). **Nothing
currently consumes `derive_traits`.** Classes and enums have no
`derive_traits` field.

### 2.7 Method-name awareness

`typeck/infer.rs:140` already special-cases `"drop"` (along with
`init`, `display`, `display_all`) as an implicit-Unit-return method for
public methods, so `def drop` on a class doesn't error out today. It is
otherwise a completely ordinary method.

## 3. Goals & Non-goals

### 3.1 Goals

- **G1** — `Drop` is a real trait. Implementing it (or deriving it, via
  compiler-synthesized field-recursive glue) causes a destructor call to
  run exactly once per owned value at the correct program point.
- **G2** — `Copy` is a marker trait. A type is Copy iff it implements /
  derives `Copy`; every Copy type must also be Clone; no Copy type may
  implement Drop (neither manually nor through a Drop field).
- **G3** — `Clone` is an explicit, always-deep copy via `x.clone`.
  `Copy: Clone` (everything Copy is automatically Clone with a trivial
  implementation).
- **G4** — `@[derive(Copy, Clone, Drop)]` and the body-level
  `derive Copy, Clone` both work for structs; `@[derive(...)]` also
  works for enums and for classes (see §8).
- **G5** — No memory leaks for `class/struct/enum` instances in the
  common path (assignment moves, end-of-scope drops). Leaks for
  conditionally-moved locals are acceptable only if behind a
  compile-time flag; the default must be correct.
- **G6** — Generics: `T: Copy`, `T: Clone`, `T: Drop` bounds work and
  are enforced at monomorphization.
- **G7** — Drop ordering is deterministic and documented: reverse
  declaration order for locals, reverse declaration order for fields.

### 3.2 Non-goals (for this doc)

- **NG1** — Drop-on-unwind. Riven has no panic strategy yet
  (`riven_panic` just `exit(101)`s — runtime.c:423-426). This doc
  assumes `panic = "abort"` and flags the follow-up in §9.
- **NG2** — `?Sized` / DST support.
- **NG3** — Async drop / `AsyncDrop`.
- **NG4** — Stabilising a `ManuallyDrop[T]` / `MaybeUninit[T]` standard
  library type. These are useful future additions but are not required
  to ship Drop.
- **NG5** — Specialisation (e.g. "blanket impl Clone where T: Copy"
  overridable by a more specific impl). §6 prescribes a compiler-built
  rule, not a user-visible blanket impl.
- **NG6** — `Pin`, `drop_in_place` on raw pointers, or any other
  unsafe-Drop interface. Unsafe Drop users write C-level `unsafe {
  free(...) }` today (see `docs/tutorial/15-unsafe.md:62-68`); that
  stays unchanged.

## 4. Drop Trait Specification

### 4.1 Declaration (compiler built-in)

```riven
trait Drop
  def mut drop
end
```

Registered in `resolve/mod.rs`; the entry already exists
(`resolve/mod.rs:150`) but carries no method-mode info. It must
express:

- exactly one required method `drop`,
- `self_mode: HirSelfMode::RefMut` (equivalent to `&mut self`),
- return type `Unit`,
- no generic parameters.

Rationale for `&mut self` (not `consume self`): Rust chose this because
consuming `self` inside `drop` would recursively need another drop call.
Riven follows the same rule.

### 4.2 User implementation

Inline in a class body:

```riven
class File
  handle: *mut Void
  def init(@handle: *mut Void) end

  impl Drop
    def mut drop
      unsafe
        riven_fclose(self.handle)
      end
    end
  end
end
```

Standalone:

```riven
impl Drop for File
  def mut drop
    unsafe
      riven_fclose(self.handle)
    end
  end
end
```

### 4.3 Semantics

- **D1** — If `impl Drop for T` exists, the compiler invokes `T::drop(&mut
  self)` exactly once when a value of type `T` is dropped, before the
  recursive drop of `T`'s fields.
- **D2** — Recursive field drops happen **after** the user-written
  `drop`, in **reverse declaration order** of fields. This matches Rust
  (self first, then fields last-to-first) and ensures user Drop code can
  still observe the fully-constructed fields.
- **D3** — Drop runs for a local exactly at the end of its lexical scope
  iff the local is still owned (i.e. not moved out). "End of scope" is
  the innermost `{ ... }`, `do ... end`, function body, match-arm body,
  or loop body in which the local was declared. Locals are dropped in
  reverse declaration order at that point. (Mirrors Rust's drop order
  and matches today's MIR pass at lower.rs:3401-3403.)
- **D4** — The return-value local is **not** dropped in the returning
  function. Ownership transfers to the caller.
- **D5** — Temporaries created within an expression are dropped at the
  end of the enclosing statement (the "end of statement" rule,
  equivalent to Rust's rule for non-bound temporaries).
- **D6** — A partially-moved value is still dropped: its still-owned
  fields are dropped in reverse declaration order; its moved fields are
  skipped (this is exactly what drop elaboration does in Rust).
- **D7** — Once dropped, a local is uninitialised. Using it is a
  compile-time error via the existing move-check machinery. Assigning
  to the same name after drop reinitialises it (just like the
  post-move reinit path already implemented at
  `borrow_check/mod.rs:466-469`).
- **D8** — **No `Drop::drop` may be called by the user.** Typeck rejects
  any method call whose resolved method is `Drop::drop`. To drop a value
  early, users call the built-in free function `drop(value)` from the
  prelude (a generic `def drop[T](x: T) {}`), which consumes the value
  and lets the normal end-of-scope mechanism kick in. Same idea as
  Rust's `std::mem::drop`.

### 4.4 Interaction with the borrow checker

- **D9** — A value being dropped is treated as taking a `&mut` borrow of
  itself at the drop point. Therefore **no other borrow of the value
  may be live at its drop point.** This prevents Rust's historical
  drop-check (`#[may_dangle]`) foot-gun in its simplest form. Because
  Riven already has NLL-style borrow expiry
  (`borrow_check/mod.rs:143`, `borrows::expire_before`), most code
  naturally satisfies this.
- **D10** — Returning a value moves it and suppresses its drop. The
  borrow checker already records this implicitly because `return`
  consumes its operand; the MIR pass already filters `return_local`
  (`lower.rs:207, 3371-3374`).
- **D11** — Conditionally-moved locals need a **drop flag**. See §7.

### 4.5 Manual invocation rules

Given the rule in D8:

- `x.drop()` — resolves to `<T as Drop>::drop`, and is **rejected**
  by typeck with E-DROP-MANUAL.
- `drop(x)` — the prelude helper, consumes `x`; typeck already supports
  generics over T and moves on consuming self (see `HirSelfMode::Consuming`
  handling at `borrow_check/mod.rs:649-664`).

Implementation: add `pub def drop[T](_x: T) {}` to the prelude, make it
trivially compile-lowered to just consume its argument (the existing
move machinery will synthesise the Drop call inside the helper's own
body, which then runs at the caller's expected point because that's
where the value was moved into the helper's parameter).

### 4.6 Generic bounds

Three bound shapes that must work:

- `T: Drop` — accept any type that implements Drop (user- or
  compiler-derived). Useful for wrapper types that want to explicitly
  document Drop-ness.
- `T: !Drop` (syntax TBD) — not in scope for this doc. Rust has no stable
  negative-Drop bound; Riven does not either.
- No bound — monomorphisation inserts the right drop glue at each
  instantiation based on the instantiated type.

`Copy` and `Drop` are **mutually exclusive at monomorphisation**: if a
generic context instantiates `T = SomeType` where `SomeType: Copy +
Drop`, that is rejected with E-COPY-DROP-CONFLICT (see §5).

## 5. Copy Trait Specification

### 5.1 Declaration (compiler built-in)

```riven
trait Copy: Clone end
```

A **marker trait**: no required methods. Having `Clone` as a super-trait
makes "`Copy: Clone`" natural (§6.3).

Registered in `resolve/mod.rs:147` (already present as a zero-method
trait; needs the `super_traits` field populated with `Clone`).

### 5.2 Which types can be Copy

A type `T` may implement `Copy` iff all of the following hold:

- **C1** — `T` does **not** implement `Drop` (user-written or
  compiler-derived), and no field's type implements `Drop`. Checked at
  the point `impl Copy for T` or `@[derive(Copy)]` is processed.
- **C2** — Every field of `T` is Copy (recursively). Includes tuple
  elements and enum-variant payloads.
- **C3** — `T` has no mutable reference field (`&mut U`). &mut is not
  Copy. (Immutable `&U` is Copy.)
- **C4** — `T` is not a builtin heap-allocated type: `String`, `Vec`,
  `Hash`, `Set`, `Option` (if payload is non-Copy), `Result`,
  `DynTrait`, `Fn`/`FnMut`/`FnOnce` closure types. The existing
  `Ty::is_copy` list in `hir/types.rs:189-221` is the source of truth;
  we extend it to consult the trait-impl table for user types.

Diagnostics:

- E-COPY-HAS-DROP: "cannot derive `Copy` for `T`: `T` implements
  `Drop`" (or "field `x: U` implements Drop").
- E-COPY-NON-COPY-FIELD: "cannot derive `Copy` for `T`: field `x: U`
  is not `Copy`".
- E-COPY-DROP-CONFLICT (generic): "type `Foo[Bar]` instantiates `T:
  Copy` with `Bar: Drop`".

### 5.3 Effect on move semantics

Today `Ty::is_copy` makes a purely structural decision
(`hir/types.rs:189`). We change it to also consult the trait-impl
table for `Ty::Class`, `Ty::Struct`, `Ty::Enum`, `Ty::Newtype`: a
user-defined type is Copy iff it has a registered `impl Copy` (nominal;
structural-Copy is nonsense for a marker trait).

Concretely:

- `Ty::is_copy(&self, resolver: &TraitResolver) -> bool` — threaded
  version used by typeck / borrow-check.
- The existing free-function `Ty::is_copy(&self) -> bool` stays for
  call-sites that don't have a resolver handy, but it returns `false`
  for all user-defined types — a conservative (Move-biased) default.
  These call sites must eventually be migrated.

Once Copy is connected, the existing flow "just works":

- `check_assign` already short-circuits on `is_copy`
  (`borrow_check/moves.rs:51`).
- `insert_drops` already filters Copy locals out of the drop set
  (`mir/lower.rs:3364-3366`).
- `MirInst::Copy` vs. `MirInst::Move` is already chosen by the MIR
  lowerer based on `is_copy`.

### 5.4 Marker-trait semantics for traits.rs

Extend `TraitResolver::check_satisfaction` at `typeck/traits.rs:86`
with a marker-trait shortcut: for a trait whose `required_methods` and
`default_methods` are both empty and whose `super_traits` are all
satisfied, satisfaction is by **explicit `impl` only** (nominal). No
structural satisfaction — "this type happens to have no methods, so
it's Copy" is obviously wrong.

### 5.5 Auto-derive rule

When the compiler sees `@[derive(Copy)]` (or body-level
`derive Copy`), it:

1. Verifies C1–C4 for the current type definition.
2. Synthesises a nominal `impl Copy for T` with no method body.
3. Also synthesises `impl Clone for T` with a `clone` method that
   simply `*self`-copies (bitwise) — see §6.5.

Failure at step 1 is a hard error, not a silent skip.

## 6. Clone Trait Specification

### 6.1 Declaration (compiler built-in)

```riven
trait Clone
  def clone -> Self
end
```

`self_mode: HirSelfMode::Ref`, return type `Self`.

Already registered (`resolve/mod.rs:148`) but with no method mode.

### 6.2 Semantics

- **Cl1** — `x.clone` returns an independently-owned deep copy of `x`.
  The original is unchanged (shared borrow).
- **Cl2** — Clone is **always explicit**. There is no implicit clone
  insertion on moves — the existing "consider cloning the value:
  `x.clone`" hint (`borrow_check/mod.rs:386-389`) is a suggestion, not an
  action.
- **Cl3** — For a type deriving `Clone`, the compiler synthesises a
  recursive field-wise clone (§6.5).
- **Cl4** — User implementations are allowed to do arbitrary work (e.g.
  deep-clone a graph, take a lock), but the result must satisfy the
  "independently owned" contract (no shared interior without explicit
  shared ownership like `Rc`).

### 6.3 `Copy: Clone`

Because `Clone` is a super-trait of `Copy`, any `impl Copy` for a type
must be accompanied by an `impl Clone`. We enforce this by having
`@[derive(Copy)]` auto-synthesise Clone as well (§5.5). A manual
`impl Copy for T` without a Clone impl is a hard error:

> E-COPY-NEEDS-CLONE: "`impl Copy for T` requires `impl Clone for T`"

### 6.4 Blanket / built-in impls

- All primitive Copy types: Clone is trivial (the value is already
  bit-copied). Codegen emits `riven_noop_passthrough` for the clone
  method — already available in the runtime (runtime.c:410-412).
- Tuples `(T, U, ...)`: Clone iff every element is Clone. Synthesised.
- Arrays `[T; N]`: Clone iff T is Clone. Synthesised as a loop.
- `Vec[T]`: Clone iff T is Clone. Runtime provides `riven_vec_clone`.
  (Not in scope for the first phase — `Vec[T].clone` already resolves
  to a missing method; we either ship the runtime helper with Drop or
  diagnose it as unimplemented.)
- `String`: already `Clone`; `String_clone` → `riven_string_from`
  (runtime.c:131-140, codegen/runtime.rs:59).
- `&T`: `(&T).clone() = *self` — references are Copy, Clone is
  trivial.
- `Option[T]`, `Result[T, E]`: Clone iff inner(s) are Clone.
- `Hash[K, V]`, `Set[T]`: Clone iff inner(s) are Clone. Needs runtime
  helper; not on the critical path for Drop.

### 6.5 Auto-derive for user types

`@[derive(Clone)]` on a struct/enum/class synthesises:

- struct `S { a: A, b: B }` →

```riven
impl Clone for S
  def clone -> S
    S { a: self.a.clone, b: self.b.clone }
  end
end
```

- enum: pattern-match the discriminant, rebuild the variant with
  `.clone` on each field.
- class: same as struct, but via the generated `new` constructor. If
  the class has a custom `init` with auto-assign args
  (`ParamInfo::auto_assign`), Clone calls `Self.new(self.a.clone,
  self.b.clone, ...)`.

All field types must themselves be Clone; if any isn't, fail with
E-CLONE-NON-CLONE-FIELD.

## 7. Drop Insertion Algorithm

The existing `insert_drops` (`mir/lower.rs:3346-3407`) is purely
syntactic and unsound for real codegen. Replace it with a MIR pass that
does **drop elaboration**, analogous to rustc's `drop_elaboration`.

### 7.1 Inputs

- `MirFunction` after lowering.
- `SymbolTable` + `TraitResolver` (to look up `impl Drop for T`).
- Per-local move facts computed during HIR borrow-check (exported:
  extend `borrow_check::BorrowChecker` to emit a per-function
  `MoveFlow` map keyed by `LocalId` — built from `DefId` via the
  existing `def_id → local_id` map already threaded by the MIR
  lowerer).

### 7.2 Pass outline

1. **Determine drop-needing locals.** A local `l: T` needs drop iff `T`
   is non-Copy and either:
   - `T` implements Drop (nominal), or
   - `T` transitively contains a field that needs drop.
   Call the resulting predicate `needs_drop(T)`. This replaces the
   ad-hoc whitelist at lower.rs:3379-3387 and crucially includes
   `String`, `Vec`, `Option`, `Result`, `Tuple`, `Array` when their
   payloads are non-Copy.

2. **Compute per-local drop state.** For each local, compute one of:
   - `AlwaysDropped` (owned at every exit that reaches a scope end):
     emit an unconditional Drop.
   - `NeverDropped` (moved on every path): no drop.
   - `MaybeDropped` (some paths move, some don't): emit a **drop
     flag** — a compiler-inserted `Bool` local initialised to `true`
     at the point the value becomes owned, set to `false` on every
     move, and checked before the Drop call: `if drop_flag_N {
     drop(local_N) }`.
   - The `MoveFlow` from the borrow checker is the source of truth;
     every `process_transfer` / `process_call_move`
     (`borrow_check/moves.rs:50-63`) becomes a "set flag to false"
     event. The conservative branch-merge
     (`moves.rs:100-114`) already gives the correct
     `MaybeDropped` classification.

3. **Insert Drop calls at scope exits.** For each scope (`ScopeKind::
   Function | Block | Loop | Closure | MatchArm` — already modelled in
   `borrow_check/regions.rs`), insert drops for its locals at:
   - the natural fall-through to the scope's successor,
   - every `return` exiting through the scope,
   - every `break` / `continue` exiting through the scope,
   - every panic edge (see §9).
   Locals are dropped in **reverse declaration order**.

4. **Lower each Drop to a runtime call.** `MirInst::Drop { local }` is
   lowered in a per-type way:
   - If `typeof(local): T` implements `Drop` nominally → call
     `T_drop(&mut local)` (the user-written or derived method).
   - Then, in reverse field order, emit `Drop { field }` for each
     non-Copy field (classes/structs/enums) — this is **drop glue**.
   - For primitives-with-heap-tail types we route to runtime helpers:
     - `Ty::String` / `Ty::Str` (owned) → `riven_string_free` (new
       helper — simple `riven_dealloc` wrapper).
     - `Ty::Vec(T)` → `riven_vec_free` (new helper — iterates,
       drops elements, frees buffer, frees the `RivenVec` struct).
     - `Ty::Option(T)` / `Ty::Result(T,E)` → branch on tag, drop
       payload, dealloc the 16-byte tagged union.
   - For `Ty::Array(T, N)` → emit a lowered loop that drops each
     element (or elide if `T` doesn't need drop).
   - For `Ty::Tuple(ts)` → drop each in reverse order.
   - For `Ty::Class` / `Ty::Struct` / `Ty::Enum` **without** user
     Drop → emit drop glue only (no user-method call), then free the
     allocation via `riven_dealloc`.
   - For raw pointers, references, and primitives → Drop is a no-op
     (they had no Drop to begin with; they shouldn't be in the drop
     set, but defence in depth).

5. **Parameters.** Parameters are owned by the callee; today
   `insert_drops` excludes them (lower.rs:3367-3370). This is **wrong**
   — a parameter taken by-value is owned by the callee and must drop on
   exit iff it wasn't moved. Fix: treat parameters the same as locals
   declared at function entry, minus the special-case of the return
   value.

6. **Temporaries.** Today `_t*` temporaries are excluded
   (`lower.rs:3376-3378`). This is also too conservative. Every
   temporary that materialises a heap value must drop at end-of-statement
   if nothing consumed it. The right answer is to tag temps with
   "expression-statement temp" vs "bound-to-local temp" in the MIR
   lowerer and let the drop pass handle each. For phase 4b, a sound
   heuristic is "drop every non-Copy temp at end-of-statement" — the
   cost is a little extra drop code per temp.

### 7.3 Invariant after the pass

- Every non-Copy local dominated by an owning-initialisation point and
  reaching a scope exit has exactly one `MirInst::Drop` on that path.
- `MirInst::Drop { local }` is never emitted when `local` is moved on
  that path, nor when the path returns `local`.
- Drop flags, when used, guard every Drop emission site.

## 8. Derive Support

### 8.1 Surface syntax

Two surface forms are accepted; they are synonyms post-parse:

```riven
# Attribute form (preferred, works everywhere):
@[derive(Copy, Clone, Debug)]
struct Point
  x: Float
  y: Float
end

# Body form (existing, struct-only today):
struct Point
  x: Float
  y: Float
  derive Copy, Clone, Debug
end
```

### 8.2 Parser changes

- Extend the `@[...]` acceptor at `parser/mod.rs:473-512` to accept
  `@[derive(...)]` and forward to struct, enum, and class parsing.
- When `@[derive(Trait1, Trait2)]` is applied, push the trait names
  into a common `derive_traits: Vec<String>` field.
- Add `derive_traits: Vec<String>` to `HirEnumDef` and `HirClassDef`
  (currently only on `HirStructDef` at `hir/nodes.rs:431`).
- Similarly extend `EnumInfo` and `ClassInfo` in
  `resolve/symbols.rs:36-56`.

### 8.3 Which traits can be derived

Phase 4d: `Copy`, `Clone`, `Drop`. Out of scope: `Debug`,
`Displayable`, `Comparable`, `Hashable`, `Error` — those are separate
derive features that can share the same mechanism later.

### 8.4 Derive lowering

A new pass `expand_derives` runs after name resolution, before
type-checking. For each type with `derive_traits`:

- `Copy`: insert a synthesised `HirImplBlock { trait_ref: "Copy",
  target_ty: T, items: [] }` after checking §5.2 constraints. Also
  auto-inserts `Clone` if not present.
- `Clone`: insert a synthesised impl with a recursive `clone` method
  (§6.5).
- `Drop`: insert a synthesised impl with `def drop { }` — a no-op user
  body. The recursive field-drop is emitted by drop glue (§7.2.4), so
  the empty user body is correct. **The main reason to derive Drop is
  to register the type as nominally `Drop` and forbid `impl Copy for
  T`.** If the user just wants field-recursive freeing, they don't
  need to derive Drop at all — the MIR pass produces drop glue for
  every type whose fields need drop.

### 8.5 Interaction with manual impls

- Explicit `impl Drop for T` + `@[derive(Drop)]` → duplicate-impl
  error.
- Explicit `impl Clone for T` + `@[derive(Clone)]` → duplicate-impl
  error (unless we spec later that derive is skipped when an explicit
  impl exists; recommendation: error, like Rust).
- `@[derive(Copy)]` + explicit `impl Drop for T` → E-COPY-HAS-DROP.

## 9. Panic / Unwind Interaction

**Current state**: `riven_panic` in runtime.c:423-426 just prints and
`exit(101)`. There is no unwinding, no panic-runtime, no landing pads.

**Decision for this phase**: assume `panic = "abort"`. Drops do **not**
run on panic. This is consistent with the current runtime and avoids
requiring landing pads in both backends.

**Follow-up**: once a panic strategy RFC lands, drop elaboration must
be extended to:

- emit a cleanup / unwind edge from every potentially-panicking call,
- run drops on the unwind path, in the same reverse order,
- gate the behaviour on a compile flag (`-C panic=unwind` /
  `-C panic=abort`).

This is a documented dependency; not blocking for Tier 1 Drop.

**Unsafe / double-panic**: if a user `drop` method itself panics under
`panic=abort`, the process aborts. Under `panic=unwind` (future), a
panic-in-drop during unwinding is a double-panic and aborts — matches
Rust.

## 10. Implementation Plan

### 10.1 Code map

| Change | File(s) |
|---|---|
| `TraitInfo { self_mode, is_marker }` | `resolve/symbols.rs:60-66` |
| Built-in trait metadata (Drop `&mut self`, Copy marker/super=Clone, Clone `&self → Self`) | `resolve/mod.rs:138-151` |
| Marker-trait satisfaction rule | `typeck/traits.rs:85-133` |
| `Ty::is_copy(resolver)` consults nominal Copy impls | `hir/types.rs:189-221` |
| Copy/Drop mutual exclusion check | new pass in `typeck/` (call it `trait_consistency`) |
| Derive expansion | new pass `resolve::expand_derives`, after resolve, before typeck |
| `@[derive(..)]` on enum/class | `parser/mod.rs:473-512`, `hir/nodes.rs` (add `derive_traits` to enum/class), `resolve/symbols.rs` |
| Drop flags + real drop-elaboration pass | rewrite `insert_drops` at `mir/lower.rs:3346-3407`; new module `mir/drop_elab.rs` |
| Emit `MoveFlow` from borrow-check | extend `borrow_check/moves.rs` public API with per-local history |
| Real codegen of `MirInst::Drop` | `codegen/cranelift.rs:692-698`, `codegen/llvm/emit.rs:790-792` |
| Runtime free helpers | `runtime/runtime.c` — add `riven_string_free`, `riven_vec_free`, `riven_option_free`, `riven_result_free` |
| Declare new runtime functions | `codegen/runtime.rs:11-26`, `codegen/llvm/runtime_decl.rs` |
| Prelude `drop[T](x: T)` | `resolve/mod.rs:173-195` (builtin fns) + a trivial MIR lowering |

### 10.2 Runtime additions

```c
/* runtime/runtime.c */

void riven_string_free(char *s) {
    if (s) free(s);
}

void riven_vec_free(RivenVec *v) {
    if (!v) return;
    /* Element-drop is emitted by codegen per-element;
       riven_vec_free assumes elements are already dropped. */
    free(v->data);
    free(v);
}

void riven_option_free(void *opt) { if (opt) free(opt); }
void riven_result_free(void *res) { if (res) free(res); }
```

Each matches the corresponding `riven_*_new` / allocation path. The
compiler emits an element-level drop loop *before* calling
`riven_vec_free` if the element type needs drop.

### 10.3 Type-check rules to add

1. At `impl Copy for T`: verify §5.2 (C1–C4) using the trait-impl
   table + field types.
2. At `impl Drop for T`: record `T: Drop` in the impl table. Reject
   `T` if `T: Copy` is already recorded. Reject generic `impl Drop
   for Foo[T]` only if neither phase can statically prove T is
   Drop-safe — for this phase, allow it and re-check at monomorphisation
   per §4.6.
3. At `impl Clone for T`: verify the method signature matches
   `def clone -> Self` with `&self` self-mode.
4. `t.drop()` where the resolved method is `<T as Drop>::drop`: reject
   with E-DROP-MANUAL.
5. Generic bound `T: Copy` at a call site where the instantiated type
   has a Drop impl: E-COPY-DROP-CONFLICT at monomorphisation.

### 10.4 Borrow-check changes

- Remove the "cloning" message's "." delimiter inconsistency in
  `borrow_check/mod.rs:386-389` once `x.clone` is a real method call
  (pure cosmetic).
- Export from `BorrowChecker` a `move_flow: HashMap<DefId, MoveFlow>`
  so the MIR drop pass can consume it. `MoveFlow` is a per-basic-block
  bit vector indicating "owned at block entry / owned at block exit".
- No changes to move tracking logic itself — the existing machinery is
  correct; we just need to persist its output for MIR consumption.

### 10.5 MIR / codegen changes

- `mir/drop_elab.rs`: new module implementing §7. `mir::lower` calls it
  after emitting the body.
- `codegen/cranelift.rs` `MirInst::Drop` handler:
  - Look up `local`'s type; call the emitted drop-glue function by name
    (`<mangled-type>_drop_glue`).
  - For built-in types, call the corresponding runtime free helper.
- `codegen/llvm/emit.rs`: same.
- A new lowering step emits per-type drop-glue functions once per type
  instantiation (class/struct/enum). These are plain MIR functions with
  `self: &mut T` and a Unit body that calls the user Drop (if any) then
  each field's drop.

### 10.6 Phasing

- **4a** — Foundations (no behaviour change for existing programs):
  - Built-in trait metadata (self_mode, marker flag, super-trait).
  - `@[derive(..)]` parsed for struct/enum/class; `derive_traits`
    threaded into HIR + symbol table.
  - Marker-trait satisfaction rule in `TraitResolver`.
- **4b** — `Drop` trait + drop glue for user class/struct/enum:
  - `impl Drop for T` registered, checked, dispatched.
  - Rewrite `insert_drops` → `drop_elab` with reverse-order field drop.
  - Real Drop codegen for `Ty::Class/Struct/Enum` (today's whitelist).
  - Runtime: no new functions yet; uses `riven_dealloc`.
  - Parameters and whitelisted temporaries start being dropped.
- **4c** — Drop flags + `Copy` marker:
  - `Copy` nominal impl recognised; `is_copy` consults trait table.
  - Copy ⊕ Drop mutual exclusion enforced.
  - Drop-flag insertion for `MaybeDropped` locals.
  - Early-exit (return/break/continue) drop paths correct.
- **4d** — `Clone` + derive:
  - `Clone` methods typechecked; derive synthesises field-wise clone.
  - `derive(Copy)` auto-derives `Clone`.
  - Built-in `drop(x)` prelude helper.
  - Extend drop elaboration to `String`, `Vec`, `Option`, `Result`,
    `Tuple`, `Array` (removes the whitelist at lower.rs:3382-3387).
  - Runtime: add `riven_string_free`, `riven_vec_free`, tagged-union
    free helpers.

Each phase should be independently landable and testable.

## 11. Test Matrix

Live in `crates/riven-core/tests/fixtures/` and unit tests in each
phase's module. Minimum coverage:

### 11.1 Drop semantics

1. **DROP-BASIC**: `impl Drop for File` runs on scope end. Assert by
   observing a side-effecting `drop` method (e.g. increments a global
   counter via FFI).
2. **DROP-ORDER-LOCALS**: locals `a`, `b`, `c` declared in that order
   → dropped `c, b, a`.
3. **DROP-ORDER-FIELDS**: struct `{ x: A, y: B }` where A and B both
   have side-effecting Drops → user-drop runs first, then `y.drop`,
   then `x.drop`.
4. **DROP-AFTER-MOVE**: `let a = f(); let b = a;` → `a`'s drop does
   **not** run at end of scope; `b`'s does.
5. **DROP-PARTIAL-MOVE**: struct destructuring moves one field; only
   the remaining fields are dropped.
6. **DROP-CONDITIONAL**: `if cond { take(x) }` → drop flag causes
   drop to skip when `cond` was true.
7. **DROP-EARLY-RETURN**: `if cond { return ... }` → locals up to the
   return are dropped in reverse order on that path.
8. **DROP-BREAK-CONTINUE**: loop body drops locals on both normal-exit
   and break-exit paths.
9. **DROP-MATCH-ARM**: match arm-local is dropped at arm end.
10. **DROP-RETURN-SUPPRESSED**: `let x = f(); x` (tail return) does
    not drop `x`.
11. **DROP-NESTED-SCOPES**: nested `do ... end` blocks drop in reverse,
    inside-out.
12. **DROP-MANUAL-REJECTED**: `x.drop()` is a compile error.
13. **DROP-PRELUDE**: `drop(x)` consumes and drops early; later use of
    `x` is E1001.
14. **DROP-NO-BORROW-CROSSING**: `let r = &x; end-of-scope drops x` is
    rejected iff `r` is still live at the drop point.

### 11.2 Copy semantics

15. **COPY-PRIMITIVE**: `let a: Int = 42; let b = a; a + b` compiles.
16. **COPY-USER-STRUCT**: `@[derive(Copy, Clone)] struct Point { x:
    Float, y: Float }` compiles; `let p2 = p1` does not invalidate
    `p1`.
17. **COPY-REJECTS-DROP-FIELD**: deriving Copy on a struct with a
    `String` field is E-COPY-NON-COPY-FIELD.
18. **COPY-REJECTS-IMPL-DROP**: `@[derive(Copy)]` + `impl Drop for T`
    is E-COPY-HAS-DROP.
19. **COPY-MUT-REF-FIELD**: struct with `&mut T` field cannot derive
    Copy.
20. **COPY-GENERIC**: `fn needs_copy[T: Copy](t: T) { let _x = t; t
    }` compiles; instantiation with `String` is rejected.

### 11.3 Clone semantics

21. **CLONE-STRING**: `let s2 = s1.clone; use(s1); use(s2)` compiles
    and produces two independent heaps.
22. **CLONE-DERIVED-STRUCT**: `@[derive(Clone)]` synthesises a
    recursive clone.
23. **CLONE-NON-CLONE-FIELD**: `@[derive(Clone)]` on a struct whose
    field isn't Clone is E-CLONE-NON-CLONE-FIELD.
24. **CLONE-OF-COPY**: a `derive(Copy)` struct is also Clone
    (auto-derived); `x.clone` works and is equivalent to `let y = x`.
25. **CLONE-TUPLE**: `(String, Int).clone` works iff all elements are
    Clone.

### 11.4 Ownership + codegen end-to-end

26. **RUNTIME-NO-LEAK**: a program that allocates and drops a
    `Vec[String]` in a loop doesn't grow RSS linearly (assert via
    `valgrind --leak-check=full` in CI, 0 definite leaks).
27. **DOUBLE-FREE-GUARD**: moved-then-dropped code path does not
    double-free (valgrind).
28. **DROP-FLAG-CODEGEN**: the conditional-move test compiles to code
    that reads the flag at runtime — assert on MIR output via
    `rivenc --emit=mir`.

## 12. Open Questions & Risks

- **OQ-1** — Drop-on-unwind: deferred. Blocks any Tier-1 work that
  assumes panics can happen in the middle of a function. Propose a
  panic-strategy RFC before writing any landing-pad code.
- **OQ-2** — `Copy` for user enums: allowed if every variant payload
  is Copy. This matches Rust. Confirm we want the same rule.
- **OQ-3** — `String` and `Vec` Drop glue: currently the whitelist at
  lower.rs:3382-3387 explicitly excludes them citing "pointers to
  static data". Part of phase 4d is removing that exclusion; we need a
  runtime invariant that `String` locals are always heap-owned
  (`riven_string_from` / `riven_string_concat` copies) before we can
  free them. Audit: verify no codegen path stores a string-literal
  pointer directly into a local typed `Ty::String`. Today the literal
  comes from `MirInst::StringLiteral` at `cranelift.rs:700-705` and
  goes into a local typed `Ty::String` — this **would** double-free
  under naive drop. Fix: either wrap `StringLiteral` with an implicit
  `String::from` call at MIR-lowering time, or type string literals as
  `Ty::Str` and force an explicit `.to_string` for owned storage. The
  latter matches Rust. This decision is load-bearing for phase 4d.
- **OQ-4** — Generic Drop impl soundness: `impl[T] Drop for Box[T]`
  needs Rust's drop-check (`#[may_dangle]`) story. For now, disallow
  type parameters escaping the Drop body via generic bounds — the
  simplest sound rule is: "generic `impl Drop` is allowed; `drop` body
  may only call `T`'s methods that are in `T`'s explicit bounds."
- **OQ-5** — Class inheritance + Drop: does the parent's Drop run after
  the child's? Today there is no destructor chain. Propose: yes,
  parent Drop runs after child Drop, matching reverse-construction
  order (init goes parent-then-child; drop goes child-then-parent).
  Needs confirmation; classes.rvn has no Drop example.
- **OQ-6** — `ManuallyDrop[T]`: not in scope. Plan to add in a future
  phase when unsafe patterns need it.
- **OQ-7** — Drop for `&mut T` aliasing: the D9 "drop takes `&mut
  self`" rule means a local being dropped must have no other live
  borrows. The NLL machinery
  (`borrow_check/borrows.rs`/`regions.rs`) already expires borrows
  early, but we should add a targeted test that tries to hold a
  `&data` across `data`'s drop point and ensure it's rejected.
- **R-1** — Risk: drop elaboration is one of the most bug-prone parts
  of a Rust-like compiler. Mitigation: land drop flags behind a flag
  (`-Z always-drop-flag`) that forces flags on every non-Copy local,
  as a conservative fallback until the flow analysis is audited.
- **R-2** — Risk: temporary lifetime rules (§D5) are subtle. Mitigation:
  start with "drop temporaries at the end of the MIR basic block that
  materialised them," which is a safe overapproximation of
  end-of-statement. Refine once tests expose divergences from
  intuitive behaviour.
- **R-3** — Risk: the current `insert_drops` whitelist is load-bearing
  for tests that assert drops happen. Deleting it will cause many
  existing MIR snapshots to change. Mitigation: update fixtures
  alongside the pass, in the same commit.
