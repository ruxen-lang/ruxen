# Spec — `std.sync.atomic`

**Source docs:**
[docs/requirements/tier1_02_concurrency.md](../../requirements/tier1_02_concurrency.md),
[docs/prompts/v1/14_phase4_concurrency.md](../../prompts/v1/14_phase4_concurrency.md).

**Status:** new in the multithreading round. Provides lock-free
primitives for the cases where `Mutex[Int]` is overkill (counters,
flags, sequence numbers).

Backed by GCC/Clang `__atomic_*` builtins on the i64/bool/usize
payload. No memory-ordering enum in this round — every operation
uses sequentially consistent ordering (`__ATOMIC_SEQ_CST`).

---

## B1 — Surface types

| Type             | Backing C type | Auto-mixins  |
|------------------|----------------|--------------|
| `AtomicI64`      | `_Atomic int64_t`  | `Send`, `Sync` |
| `AtomicBool`     | `_Atomic _Bool`    | `Send`, `Sync` |
| `AtomicUsize`    | `_Atomic uint64_t` | `Send`, `Sync` |

Each is a struct wrapper — not a primitive. They live on the heap
(boxed) so a pointer suffices on the FFI boundary, matching the
i64-payload ABI.

## B2 — Constructors

| Call                              | Returns        |
|-----------------------------------|----------------|
| `AtomicI64.new(v: Int)`           | `AtomicI64`    |
| `AtomicBool.new(v: Bool)`         | `AtomicBool`   |
| `AtomicUsize.new(v: USize)`       | `AtomicUsize`  |

## B3 — Load / store

| Call                          | Returns  |
|-------------------------------|----------|
| `a.load()`                    | inner T  |
| `a.store(v: T)`               | `()`     |

Load is `__atomic_load_n(..., __ATOMIC_SEQ_CST)`. Store is the
matching `__atomic_store_n`.

## B4 — Fetch-add / fetch-sub (AtomicI64, AtomicUsize)

| Call                          | Returns       |
|-------------------------------|---------------|
| `a.fetch_add(delta: T)`       | previous value |
| `a.fetch_sub(delta: T)`       | previous value |

Returns the **previous** value (matches `__atomic_fetch_add`'s
return). For "new value, please" use the returned + delta.

## B5 — Compare-and-swap

| Call                                        | Returns                |
|---------------------------------------------|------------------------|
| `a.compare_and_swap(current: T, new: T)`    | `T` (the actual prior) |

Implemented via `__atomic_compare_exchange_n`. If the prior value
equals `current`, the new value is stored and `current` is
returned. Otherwise the prior value is returned (no store).

`a.compare_and_swap(c, n) == c` ⟺ swap succeeded.

## B6 — Bitwise ops (AtomicBool only)

| Call                          | Returns       |
|-------------------------------|---------------|
| `a.fetch_and(v: Bool)`        | previous Bool |
| `a.fetch_or(v: Bool)`         | previous Bool |

## B7 — Auto-mixins

All three types auto-derive `Send` and `Sync`. This is hardcoded in
the resolver alongside the `String`/`Int` cases — atomics are
explicitly cross-thread safe by definition.

`SharedSync[AtomicI64]` is the canonical "shared counter" pattern;
test coverage in B9 below.

## B8 — Single-thread sanity round-trip

**Given** `let a = AtomicI64.new(10)`
**Then**
- `a.load()` → 10
- `a.fetch_add(5)` → 10 (prior); `a.load()` → 15
- `a.compare_and_swap(15, 0)` → 15; `a.load()` → 0
- `a.compare_and_swap(15, 99)` → 0 (failed); `a.load()` → 0

## B9 — Multi-thread fetch_add stress

**Given** `let c = SharedSync.new(AtomicI64.new(0))`,
100 threads each call `c.deref().fetch_add(1)` 1000 times
**Then** `c.deref().load()` returns exactly 100_000.

(Same stress as `B17` in sync.spec.md but without the `Mutex`
indirection — exercises the atomic fast path directly.)

---

## Pin tests

| Behaviour | Test fn                                     | File                        |
|-----------|---------------------------------------------|-----------------------------|
| B1, B2    | `atomic_constructors_resolve`               | `std_sync_runtime.rs`       |
| B3        | `atomic_load_store_round_trip`              | `std_sync_runtime.rs`       |
| B4        | `atomic_fetch_add_sub_round_trip`           | `std_sync_runtime.rs`       |
| B5        | `atomic_compare_and_swap_round_trip`        | `std_sync_runtime.rs`       |
| B6        | `atomic_bool_fetch_and_or`                  | `std_sync_runtime.rs`       |
| B7        | `atomic_auto_derives_send_sync`             | `concurrency_markers.rs`    |
| B8        | e2e `cases/543_atomic_round_trip.rvn`       | release-e2e                 |
| B9        | e2e `cases/544_atomic_fetch_add_stress.rvn` | release-e2e                 |

---

## Out of scope

- `Ordering` enum (Relaxed, Acquire, Release, AcqRel, SeqCst).
  Always SeqCst this round.
- `AtomicPtr[T]` — needs raw-pointer primitive (deferred with the
  rest of the unsafe machinery).
- `AtomicI32`, `AtomicU8`, etc. — only i64/bool/usize ship here.
- Wait/notify (`futex`-backed) — separate concern.
