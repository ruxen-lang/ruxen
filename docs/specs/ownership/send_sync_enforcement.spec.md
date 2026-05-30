# Spec — `Send` / `Sync` enforcement (E1100–E1102)

**Source docs:**
[docs/requirements/tier1_02_concurrency.md](../../requirements/tier1_02_concurrency.md),
[docs/specs/stdlib/sync.spec.md](../stdlib/sync.spec.md) §B12–B16,
[docs/specs/ownership/borrow-check.spec.md](borrow-check.spec.md).

**Status:** new — phase 3 of the multithreading round. The mixins
themselves are pre-existing (registered as built-in traits since
before this work — verified by `register_builtins_includes_send_and_sync_traits`).
This spec covers the *enforcement* — borrow-check + construction-site
checks that reject programs which carry non-`Send` values across
thread boundaries.

`Send`/`Sync` are **manual markers** in v1: user classes opt in by
writing `include Send` in the class body. Built-in containers
auto-derive per the rules in `sync.spec.md` B12.

---

## B1 — `Send` / `Sync` registered as built-in marker mixins

**Given** any module
**When** a class body writes `include Send` or `include Sync`
**Then** the mixin resolves cleanly. Both mixins require no
methods — they are pure markers.

(Already shipped — pinned for completeness.)

## B2 — Built-in `Send` / `Sync` auto-derivation rules

The compiler treats the following types as `Send`-and-`Sync` without
the user needing to write `include`:

| Type                          | `Send` | `Sync` | Rule                                         |
|-------------------------------|:------:|:------:|----------------------------------------------|
| `Int`, `Bool`, `Float`, `Char`| ✅     | ✅     | Primitives are pure data                     |
| `String`                      | ✅     | ✅     | Immutable + reference-counted                |
| `Array[T]`                    | T?     | T?     | Iff T: Send/Sync                             |
| `Option[T]`                   | T?     | T?     | Iff T: Send/Sync                             |
| `Result[T, E]`                | T?, E? | T?, E? | Iff both T and E: Send/Sync                  |
| `Box[T]`                      | T?     | T?     | Iff T: Send/Sync                             |
| `HashMap[K, V]`, `HashSet[T]` | K?, V? | K?, V? | Iff all type params: Send/Sync               |
| `Mutex[T]`                    | T?     | T?     | Iff T: Send (Mutex makes Sync from Send)     |
| `SharedSync[T]`               | T?     | T?     | Iff T: Send                                  |
| `JoinHandle[T: Send]`         | ✅     | ✅     | T: Send required at construction (B3)        |
| `AtomicI64`/`Bool`/`Usize`    | ✅     | ✅     | Lock-free; correct by definition             |
| `Sender[T: Send]`             | ✅     | —      | Cross-thread by design; not Sync (1-writer)  |
| `Receiver[T: Send]`           | ✅     | —      | Cross-thread by design; not Sync (1-reader)  |
| `MutexGuard[T]`               | ❌     | ❌     | RAII tied to acquiring thread                |
| `ReadGuard[T]`/`WriteGuard[T]`| ❌     | ❌     | Same as MutexGuard                           |

User classes are **NOT** auto-derived. A class wrapping only `Send`
fields is still not `Send` until the user writes `include Send`.

## B3 — `Mutex.new(value)` rejects non-Send T (E1101)

**Given** a user class `Foo` without `include Send`
**When** `Mutex.new(Foo.new)` is compiled
**Then** the typeck (or borrow-check) phase emits E1101 at the call
site:
```
[E1101] error: cannot construct Mutex[Foo] — type parameter T must
implement Send, but Foo does not
note: add `include Send` to Foo if it is safe to share across threads
  --> file.rx:NN:NN
   | Mutex.new(Foo.new)
   |           ^^^^^^^
```

The check fires at the `Mutex.new` call site, not at the
`Mutex[T]` type-resolution site (so `let m: Mutex[Foo]` without
construction doesn't fire — it's the construction that crosses the
safety boundary).

## B4 — `SharedSync.new(value)` rejects non-Send T (E1102)

Same shape as B3 but with diagnostic E1102. SharedSync requires
`T: Send` (not `Sync`) — the wrapper itself doesn't allow mutable
sharing, so `Sync` isn't required of T; the cross-thread move is the
constraint.

## B5 — `Channel`/`Sender`/`Receiver[T]` reject non-Send T

`channel[T]()` and any direct construction of `Sender[T]`/
`Receiver[T]` rejects T without Send. Diagnostic reuses E1101
(same shape as Mutex — "type parameter T must implement Send").

## B6 — `Thread.spawn(closure)` rejects non-Send captures (E1100)

**Given** `let foo: Foo = ...` where `Foo` doesn't implement Send
**When** `Thread.spawn do || foo.bar end` is compiled
**Then** the borrow checker computes the closure's capture set,
finds `foo` is captured by move (or by reference), checks its
type's mixin set, and on `Send` absent emits:
```
[E1100] error: cannot capture `foo` of type Foo across thread boundary
  — Foo does not implement Send
note: add `include Send` to Foo if it is safe to share across threads
  --> file.rx:NN:NN
   | Thread.spawn do || foo.bar end
   |                    ^^^
```

The check fires **only** at `Thread.spawn` call sites — not at
arbitrary closure construction. Closures used in `Array.each`,
`Array.map`, etc. don't trigger the check.

## B7 — Capture-by-reference check semantics

When a closure captures `foo` by reference (rather than by move),
the check requires `&Foo: Send` — which for v1 means `Foo: Sync`.
This is because `&T: Send` iff `T: Sync` (Rust's rule, kept for
v1 as the natural fit).

E1100 diagnostic distinguishes by-ref vs by-move in the note:
```
note: foo is captured by reference; the closure requires `&Foo: Send`,
which means Foo must implement Sync
```

## B8 — Multiple captures: every capture checked

**Given** a closure capturing `foo` (Send) and `bar` (not-Send)
**Then** E1100 fires for `bar` only. `foo` is fine.

The check is per-capture; one captured non-Send doesn't poison the
others' diagnostics. (Distinct error spans per offending capture.)

## B9 — `Mutex[T]: Send` auto-derive transitivity check (E1101)

The auto-derive rule for `Mutex[T]: Send` is "iff `T: Send`". When
the user constructs `Mutex.new(value)`, the check from B3 fires on
T's Send-ness directly — same diagnostic. No new diagnostic needed
for the transitivity case; B3 covers it.

## B10 — User opt-in marker pattern

```rx
class MyData
  include Send         # marker — no methods required
  include Sync         # marker — no methods required
  ...
end
```

After these markers, `MyData` is treated as `Send + Sync` for all
B3/B4/B5/B6 checks.

## B11 — Negative `include` (opt-out / negative trait)

The parser already supports `include !Send` (negative include) per
the existing `negative_trait` flag in `resolve/items.rs`. Effect:
the class is explicitly NOT `Send`, even if all its fields are. This
overrides any auto-derive rule for the class. The borrow check
treats `!Send` identically to "no Send mixin at all" for the purposes
of B3-B8.

(Already shipped — pinned here.)

## B12 — `unsafe impl Send` escape hatch

```rx
class WrapsRawPointer
  include unsafe Send   # asserts safety manually
end
```

The `unsafe` qualifier suppresses any auto-derive check (e.g.
"this class contains a non-Send field, so it cannot be Send"). For
v1, since user classes have no auto-derive in the first place, this
matters only when the class wraps a built-in container that would
otherwise prevent Send derivation — covered by the existing
`manual_send` flag in `resolve/items.rs`.

## B13 — Diagnostic structure

All E1100/E1101/E1102 diagnostics share a common note:
> note: add `include Send` to TYPE_NAME if it is safe to share across
> threads. See docs/specs/ownership/send_sync_enforcement.spec.md.

Diagnostic codes are reserved in `compiler/ruxen_core/src/diagnostics/`:
- E1100 — non-Send closure capture across thread boundary
- E1101 — `Mutex.new` / `Sender`/`Receiver`/`channel[T]` with non-Send T
- E1102 — `SharedSync.new` with non-Send T

## B14 — Performance: check fires only for thread-boundary call sites

The borrow check pass walks closures regardless. The
thread-boundary check (`Thread.spawn`) adds at most one per-capture
mixin-membership lookup. Mixin membership is a HashSet check —
constant time. No measurable performance hit.

---

## Pin tests

| Behaviour | Test fn                                              | File                          |
|-----------|------------------------------------------------------|-------------------------------|
| B1        | `send_sync_marker_mixins_register_and_resolve`       | `concurrency_markers.rs`      |
| B2        | `builtin_containers_auto_derive_send_sync_when_t_is` | `concurrency_markers.rs`      |
| B3        | `mutex_new_rejects_non_send_t_e1101`                 | `concurrency_negative.rs`     |
| B4        | `sharedsync_new_rejects_non_send_t_e1102`            | `concurrency_negative.rs`     |
| B5        | `channel_rejects_non_send_t_e1101`                   | `concurrency_negative.rs`     |
| B6        | `thread_spawn_rejects_non_send_capture_e1100`        | `concurrency_negative.rs`     |
| B7        | `thread_spawn_capture_by_ref_requires_sync_e1100`    | `concurrency_negative.rs`     |
| B8        | `thread_spawn_emits_one_diagnostic_per_bad_capture`  | `concurrency_negative.rs`     |
| B9        | covered by B3 (transitivity)                         | —                             |
| B10       | `user_class_with_include_send_passes_thread_spawn`   | `concurrency_markers.rs`      |
| B11       | `negative_include_send_rejects_thread_spawn`         | `concurrency_negative.rs`     |
| B12       | `unsafe_include_send_overrides_auto_derive`          | `concurrency_markers.rs`      |
| B13, B14  | covered by B3-B8 (diagnostic structure + perf)       | —                             |

---

## Out of scope (later)

- **Auto-derive for user classes.** Walks the field set, derives
  Send iff every field is Send. Requires a fixpoint pass over the
  class graph (mutual references) — substantial separate piece.
- **`!Sync` vs `!Send` separation diagnostics.** Today both `Send`
  and `Sync` violations produce similar wording; v2 may want
  distinct messages.
- **Suggesting which field is the problem.** The current diagnostic
  names the captured value's type; a richer diagnostic would point
  at the specific non-Send field. Future polish.
- **Cross-crate Send/Sync propagation.** All current stdlib types
  are in-tree, so this works in v1. Cross-crate inference is a v2
  concern.
