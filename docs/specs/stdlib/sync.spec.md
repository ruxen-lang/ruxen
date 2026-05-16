# Spec — `std.sync`

**Source docs:**
[docs/requirements/tier1_02_concurrency.md](../../requirements/tier1_02_concurrency.md).

**Status:** typeck surface shipped Phase 1 (resolver + typeck);
runtime support is **`Thread.sleep` + `Thread.yield_now` only**.
Everything else (`Mutex`, `SharedSync`, `JoinHandle.join`, `Thread.spawn`)
types correctly but has no runtime impl yet — Phase 4 lands the
remainder.

`std.sync` is the concurrency surface.  The v1 cut prioritises the
typeck contract so user code can be written and reviewed before the
runtime work ships.

---

## B1 — Thread surface types

The following types are resolvable at the type level:

| Type                    | Role                                        |
|-------------------------|---------------------------------------------|
| `Thread`                | Spawn / utility namespace                   |
| `JoinHandle[T]`         | Handle returned by `Thread.spawn`           |
| `ThreadId`              | Opaque thread identifier                    |
| `Mutex[T]`              | Mutual-exclusion wrapper                    |
| `MutexGuard[T]`         | RAII guard returned by `Mutex.lock`         |
| `SharedSync[T]`         | Atomic reference-counted shared owner       |
| `PoisonError`           | Error from a poisoned mutex                 |
| `ThreadPanic`           | Error from a panicked spawned thread        |

All seven names are importable via `use std.sync.{...}`.  A program
that constructs and threads these types through method calls
typechecks cleanly.

## B2 — `Thread.sleep(ns: Int)` runtime helper

**Given** `Thread.sleep(0)`
**When** the program runs
**Then** the call returns immediately (no error).  Larger values
sleep the calling thread for `ns` nanoseconds via the POSIX
`nanosleep` runtime helper.

## B3 — `Thread.yield_now()` runtime helper

**Given** `Thread.yield_now()`
**Then** the runtime calls `sched_yield(3)` and returns `()`.

## B4 — `Mutex[T]` typeck signatures

The following method signatures resolve cleanly:

| Call                                    | Returns                          |
|-----------------------------------------|----------------------------------|
| `Mutex.new(value: T)`                   | `Mutex[T]`                       |
| `m.lock()`                              | `Result[MutexGuard[T], PoisonError]` |
| `m.lock!()`                             | `MutexGuard[T]` (panics on poison) |
| `m.try_lock()`                          | `Option[MutexGuard[T]]`          |
| `m.into_inner()`                        | `Result[T, PoisonError]`         |
| `g.deref()` / `g.deref_var()`           | `&T` / `&var T`                  |

**Runtime is not yet wired.**  Calling `mutex.lock()` at runtime
panics with "not implemented" until the Phase 4 mutex helpers land.

## B5 — `SharedSync[T]` typeck signatures

| Call                              | Returns               |
|-----------------------------------|-----------------------|
| `SharedSync.new(value: T)`        | `SharedSync[T]`       |
| `shared.clone()`                  | `SharedSync[T]`       |
| `shared.strong_count()`           | `USize`               |
| `shared.weak_count()`             | `USize`               |
| `shared.deref()`                  | `&T`                  |

**Runtime is not yet wired** (same caveat as B4).

## B6 — `Thread.spawn(closure) -> JoinHandle[T]` typeck

```riven
let handle: JoinHandle[Int] = Thread.spawn({ || 42 })
let joined: Result[Int, ThreadPanic] = handle.join()
```

**Runtime is not yet wired.**  The closure is type-checked and the
return type flows through, but executing this at runtime hits a
"thread spawn not implemented" panic until Phase 4.

---

## Pin tests

| Behaviour | Test fn                                       | File                          |
|-----------|-----------------------------------------------|-------------------------------|
| B1, B4-B6 | `std_sync_concurrency_surface_typechecks_cleanly` | `std_use_resolution.rs`   |
| B2, B3    | `std_sync_thread_sleep_and_yield_round_trip`  | `std_use_resolution.rs`       |

---

## Gaps

Massive intentional gap: the **runtime** for B4-B6 isn't wired
(Phase 4 work).  The typeck contract is pinned so user-facing code
shape stabilises now; the runtime fills in later.

When Phase 4 lands:
- `Mutex.lock()` should panic on poison via a deterministic check,
  not segfault.
- `SharedSync.clone()` should atomically increment via `__atomic_fetch_add`.
- `Thread.spawn` should call `pthread_create(3)` and propagate the
  closure's return value back via `JoinHandle.join`.

Each will get its own behaviour + pin test as it ships.

---

## Out of scope (v2)

- `RwLock[T]` — separate type, not yet typeck'd.
- `Channel[T]` — MPSC channel, not yet typeck'd.
- `AtomicI64` / `AtomicBool` / typed atomics.
- `Barrier`, `Semaphore`, `CondVar`.
- `thread_local!` storage.
- Async runtime (entire `std.future` surface lives in Phase 4 prompts).
