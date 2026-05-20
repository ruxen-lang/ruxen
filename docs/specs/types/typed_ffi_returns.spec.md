# Spec — Typed FFI returns for generic stdlib classes

**Source docs:**
[docs/specs/stdlib/sync.spec.md](../stdlib/sync.spec.md),
[docs/specs/stdlib/atomic.spec.md](../stdlib/atomic.spec.md),
[docs/specs/codegen/ffi.spec.md](../codegen/ffi.spec.md).

**Status:** new — phase 2 of the multithreading round.

The FFI boundary in Riven carries every value as `int64_t` per the
existing ABI (see `library/std/array/src/lib.rvn` for the canonical
"generic-stripping at the call site" comment). Today, `Array[T]`'s
constructor `Array.new()` is declared as `-> Int` in its lib block
but **typeck reports the result as `Array[T]`** because
`resolve_type_expr` has a hardcoded match arm that lifts `Array` to
`Ty::Array(elem)`. Every other generic stdlib class added since
(`Mutex[T]`, `MutexGuard[T]`, `SharedSync[T]`, `JoinHandle[T]`,
`AtomicI64`, `AtomicBool`, `AtomicUsize`, `Sender[T]`, `Receiver[T]`)
ships its constructors as `-> Int` and the typeck mirrors that
literally — so `let m = Mutex.new(7)` binds `m: Int` and `m.lock_raw`
returns `Int`, breaking the ergonomic surface.

This spec defines the lift: which lib decls trigger it, what type
the call site reports, and how T is bound.

---

## B1 — `def self.new as "..."(value: T) -> Int` lifts to `-> Self[T]`

**Given** a generic class `Foo[T]` with a lib decl
`def self.new as "riven_foo_new"(value: T) -> Int` inside its
`lib "..."` block
**When** the user writes `let f = Foo.new(value)`
**Then** typeck reports `f: Foo[T]` (T inferred from `value`'s
type), not `f: Int`. The MIR call site still passes the i64 to the
C symbol; only the *reported* surface type changes.

## B2 — Generic class instance methods returning `-> Int` lift to the
class-relative surface return

**Given** a generic class `Foo[T]` with a lib decl
`def bar as "riven_foo_bar"(self) -> Int` inside its lib block,
AND a co-located lib decl `def baz as "riven_foo_baz"(self) -> Bar[T]`
**When** the user writes `f.bar` on `f: Foo[T]`
**Then** the return type is reported as the **declared** surface
type. If the lib decl says `-> Int`, the return is Int (B2 doesn't
auto-lift). If it says `-> Bar[T]`, the return is `Bar[T]` and T is
substituted from the receiver.

The asymmetry vs. B1 is deliberate: `.new` always returns the
constructed class (universal pattern), so the lift is safe to bake
in. Instance-method return types are user-chosen and may legitimately
be raw Int (e.g., `riven_thread_join` returning the closure's i64
result before unwrapping to `Result[T, ThreadPanic]`).

Authors who want a typed return write it explicitly:

```rvn
class Mutex[T]
  lib "runtime/mutex.c"
    def self.new as "riven_mutex_new"(initial: T) -> Mutex[T]
    def lock_raw as "riven_mutex_lock"(self) -> MutexGuard[T]
    def get as "riven_mutex_guard_get"(self) -> T  # on MutexGuard
  end
end
```

The lib decl is the source of truth. Codegen treats every declared
type as i64 at the FFI boundary (per existing ABI); typeck reports
the structured type to the user.

## B3 — T binding from constructor arg type

**Given** `Mutex.new(7)` where `7: Int`
**Then** the inferred type of the result is `Mutex[Int]`.

**Given** `SharedSync.new("hi")` where `"hi": String`
**Then** the result is `SharedSync[String]`.

When the class has multiple type params (`Result[T, E]`,
`HashMap[K, V]`), T is bound positionally from arg order matching
the class's declared generic params.

## B4 — T binding from class-generic context

**Given** an instance method declared as `def baz as "..."(self) -> T`
inside `class Foo[T]`
**When** called on `f: Foo[String]`
**Then** the return type is `String`.

The `T` in the return type is the class's T, not a method-level
generic.

## B5 — Lib decls declaring `Self`-typed return are class-relative

Within `class Foo[T]`, a return type of `Self` or `Foo[T]` in a lib
decl is identical to writing `Foo[T]` — typeck doesn't care about
the spelling. Codegen emits the same i64-returning C call.

## B6 — Chained access typechecks

**Given** the typed Mutex decl from B2
**Then** the following typechecks:
```rvn
let m = Mutex.new(7)           # m: Mutex[Int]
let g = m.lock_raw             # g: MutexGuard[Int]
let v = g.get                  # v: Int
g.set(99)                      # OK: set takes Int
```

The lift propagates through the chain without any explicit casts.

## B7 — Drop emission honours the typed return

**Given** `Mutex.new(...)` returns `Mutex[T]` typed
**Then** the MIR drop pass emits a call to `Mutex_drop` (mapped to
`riven_mutex_drop` via the FFI alias) on scope exit. The lift
preserves Drop semantics — the codegen knows the value is a
`Mutex[T]` handle and dispatches drop on the class, not on Int.

## B8 — Lift applies to: `Mutex[T]`, `MutexGuard[T]`, `SharedSync[T]`, `JoinHandle[T]`, `AtomicI64`, `AtomicBool`, `AtomicUsize`, `Sender[T]`, `Receiver[T]`, plus future generic stdlib classes

The list above covers the multithreading surface. Future generic
stdlib classes get the lift automatically via the mechanism in B9
— no resolver-side registration needed.

## B9 — Mechanism

Implemented via the **lib decl is the source of truth** path (B2
generalised). Two places in the compiler change:

1. **`resolve/ffi_registration.rs`** — when a class body's `lib` block
   is being processed, push the class's generic-param names + a
   `Self` alias into a fresh class scope so the resolver can spell
   structured return types (`-> Mutex[T]`, `-> MutexGuard[T]`,
   `-> T`, `-> Self`) inside the lib decls. Without this scope push,
   the type resolver sees `T` as an undefined identifier and emits
   E0700.

2. **`resolve/bootstrap_merge.rs`** — when merging bootstrap programs,
   process class lib decls in a SECOND walk after every class type
   in every bootstrap program is registered in `type_registry`. This
   lets `class Mutex[T]; def lock_raw -> MutexGuard[T]` resolve
   `MutexGuard[T]` even when `class MutexGuard[T]` appears later in
   the same file. The first walk now only registers class TYPES;
   the second walk processes lib decls.

3. **`typeck/infer.rs::subst_ty`** — recurse into `Ty::Class /
   Struct / Enum / Result / Tuple` so `MutexGuard[T]` substitutes to
   `MutexGuard[Int]` when T is bound to Int from the receiver
   (`m: Mutex[Int]; m.lock_raw -> MutexGuard[T]`). The pre-existing
   substitution helper only walked `Ref / Option / Array` arms.

The lift is opt-in EXPLICITLY at the lib decl: authors declare the
typed return they want (`-> Mutex[T]`, `-> T`, `-> Self`, etc.) and
the compiler reports that surface type at every call site. The
codegen still treats the return as i64 at the C ABI per
`ty_to_cranelift` (which maps every pointer-shape / class / type
param to I64) — B11. The automatic `def self.new -> Int` → `Self[T]`
lift described in earlier drafts of this spec is **not implemented**:
all `def self.new` decls in the stdlib that should lift now declare
their structured return explicitly. The existing hardcoded
constructor-lift arms (`Mutex.new`, `Arc.new`, `SharedSync.new`)
in `typeck/method_resolvers/mod.rs` remain as a safety net for any
external code path that calls `.new` through `resolve_method_call`
without the lib decl's structured return — they're idempotent with
the lib decl now declaring the same type.

## B10 — Negative: lift does NOT apply to non-generic class
constructors returning `-> Int`

**Given** a hypothetical non-generic class
`class Plain; lib { def self.new as "..." -> Int } end`
**Then** the return remains `Int`. The lift only triggers when the
class has at least one generic parameter — non-generic class
constructors returning Int are a deliberate "give me the i64 handle"
escape hatch.

## B11 — Negative: lift does NOT change the FFI symbol's signature

The C symbol `riven_mutex_new(int64_t initial)` is called the same
way before and after the lift. Cranelift sees the same arg types,
the same return type (i64). Only typeck's reporting changes.

## B12 — Negative: bare `def self.new() -> Int` outside a class

A top-level `def new(...) -> Int` (or a free-fn shim outside any
class) does NOT get the lift. The lift is keyed on
`<class>.<method>` where `<class>` is a registered generic class.

---

## Pin tests

| Behaviour | Test fn                                              | File                          |
|-----------|------------------------------------------------------|-------------------------------|
| B1, B3    | `mutex_new_lifts_to_mutex_of_t`                      | `typed_ffi_returns.rs`        |
| B2        | `instance_method_return_type_honours_declaration`    | `typed_ffi_returns.rs`        |
| B4        | `class_generic_t_substitutes_in_return`              | `typed_ffi_returns.rs`        |
| B5        | `self_in_return_position_resolves_to_class_t`        | `typed_ffi_returns.rs`        |
| B6        | e2e `cases/710_mutex_lock_unlock_typed_surface.rvn`  | release-e2e                   |
| B7        | `mutex_drop_emitted_on_typed_scope_exit`             | `typed_ffi_returns.rs`        |
| B8        | covered by 710 (Mutex), 711 (SharedSync), 712 (Atomic) | release-e2e                 |
| B9        | covered by integration tests B1-B7                   | —                             |
| B10       | `non_generic_class_constructor_returns_int`          | `typed_ffi_returns.rs`        |
| B11       | `ffi_signature_unchanged_after_lift`                 | `typed_ffi_returns.rs`        |
| B12       | `top_level_def_new_does_not_lift`                    | `typed_ffi_returns.rs`        |

---

## Out of scope (later)

- **Type-level generic substitution beyond positional matching.**
  `HashMap[K, V]` with `HashMap.from_iter(iter)` doesn't bind K/V
  positionally — needs HRTBs (#08) or richer constructor inference.
- **Multiple constructors with conflicting T binding rules.**
  Today the lift uses the first constructor's first arg. Classes
  needing multiple constructors (`from_iter`, `with_capacity`, etc.)
  layer named constructors on top.
- **Lift on enums and structs.** This spec covers classes only; the
  enum/struct path stays raw-Int because their FFI shape differs.
