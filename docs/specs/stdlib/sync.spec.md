# Spec — `std.sync`

**Source docs:**
[docs/requirements/tier1_02_concurrency.md](../../requirements/tier1_02_concurrency.md),
[docs/prompts/v1/14_phase4_concurrency.md](../../prompts/v1/14_phase4_concurrency.md).

**Status:** typeck surface shipped Phase 1; **runtime + safety markers
land in this round** (post-#06.95 multithreading). The earlier
"`Thread.sleep`/`yield_now` only" cap is lifted: `Thread.spawn`,
`JoinHandle.join`, `Mutex` lock/unlock, `SharedSync` refcounting,
and the manual `Send`/`Sync` markers all ship here.

`std.sync` is the concurrency surface — OS-thread spawn/join, mutual
exclusion, and atomically-refcounted shared ownership. Message
passing lives in [`channel.spec.md`](channel.spec.md), atomics in
[`atomic.spec.md`](atomic.spec.md), reader-writer locks in
[`rwlock.spec.md`](rwlock.spec.md), one-shot init in
[`once.spec.md`](once.spec.md).

---

## B1 — Thread surface types

The following types are resolvable at the type level via
`use std.sync.{...}`:

| Type                    | Role                                            |
|-------------------------|-------------------------------------------------|
| `Thread`                | Spawn / utility namespace                       |
| `JoinHandle[T: Send]`   | Handle returned by `Thread.spawn`               |
| `ThreadId`              | Opaque thread identifier                        |
| `Mutex[T]`              | Mutual-exclusion wrapper                        |
| `MutexGuard[T]`         | RAII guard returned by `Mutex.lock`             |
| `SharedSync[T]`         | Atomic reference-counted shared owner (Arc)     |
| `PoisonError`           | Error from a poisoned mutex                     |
| `ThreadPanic`           | Error from a panicked spawned thread            |

## B2 — `Thread.sleep(d: &Duration)` runtime helper

**Given** `Thread.sleep(Duration.from_millis(0))`
**When** the program runs
**Then** the call returns immediately. Larger values sleep the
calling thread via POSIX `nanosleep`.

(Already shipped Phase 1; pinned here for completeness.)

## B3 — `Thread.yield_now()` runtime helper

**Given** `Thread.yield_now()`
**Then** the runtime calls `sched_yield(3)` and returns `()`.

(Already shipped Phase 1.)

## B4 — `Thread.current_id() -> ThreadId`

**Given** a running thread
**Then** `Thread.current_id()` returns a `ThreadId` that is stable
for the lifetime of that thread and distinct from every other
live thread's id. Backed by `pthread_self(3)` cast to `Int`.

## B5 — `Thread.spawn(closure) -> JoinHandle[T]` runtime

**Given** `let h = Thread.spawn do || compute() end` where
`compute()` returns `T`
**When** the surrounding scope continues
**Then** a new OS thread is created via `pthread_create(3)` and
begins executing the closure. The handle holds an opaque pthread
identifier plus a heap-allocated result slot.

**Closure capture rule (E1100):** every value captured by the
closure must satisfy `Send`. The borrow checker rejects captures
that don't with diagnostic E1100. Mixin discipline is **manual**
in this round — the user writes `include Send` on classes they
want to ship across thread boundaries (see B12 below).

## B6 — `JoinHandle.join() -> Result[T, ThreadPanic]`

**Given** a `JoinHandle[T]`
**When** `.join()` is called
**Then** the caller blocks until the spawned thread exits.
- Normal exit → `Ok(T)` carrying the closure's return value (passed
  back via the heap result slot).
- Spawned thread panicked → `Err(ThreadPanic)` carrying the panic
  message.

Calling `.join()` twice on the same handle is an error (E1104,
"handle already joined"). The handle's Drop frees the result slot
if `.join()` was never called.

## B7 — `Mutex[T]` runtime

| Call                                    | Returns                              |
|-----------------------------------------|--------------------------------------|
| `Mutex.new(value: T)`                   | `Mutex[T]`                           |
| `m.lock()`                              | `Result[MutexGuard[T], PoisonError]` |
| `m.lock!()`                             | `MutexGuard[T]` (panics on poison)   |
| `m.try_lock()`                          | `Option[MutexGuard[T]]`              |
| `m.into_inner()`                        | `Result[T, PoisonError]`             |

`Mutex[T]` requires `T: Send` (E1101 on instantiation otherwise).
The Mutex stores `{ pthread_mutex_t, int64_t payload }`. The payload
carries either T inline (if T fits in 8 bytes per the existing ABI)
or a heap pointer to T. The same i64-payload pattern as
`Array[T]`/`Vec[T]` — see `library/std/array/src/lib.rx` for the
canonical comment.

**Poison rule:** if the thread holding the lock panics, the mutex
is marked poisoned. Subsequent `lock()` returns `Err(PoisonError)`
until `clear_poison()` is called explicitly.

## B8 — `MutexGuard[T]` deref + Drop

| Call                            | Returns       |
|---------------------------------|---------------|
| `g.deref()`                     | `&T`          |
| `g.deref_var()`                 | `&var T`      |

`MutexGuard[T]` is a Drop type. Going out of scope releases the
mutex via `pthread_mutex_unlock(3)`. The codegen drop emission
must fire on every scope exit (normal + panic unwind, once unwind
support lands; for now panic = abort, so scope exit suffices).

## B9 — `SharedSync[T]` runtime

| Call                              | Returns                |
|-----------------------------------|------------------------|
| `SharedSync.new(value: T)`        | `SharedSync[T]`        |
| `shared.clone()`                  | `SharedSync[T]`        |
| `shared.strong_count()`           | `USize`                |
| `shared.deref()`                  | `&T`                   |

`SharedSync[T]` requires `T: Send` (E1102 otherwise — note: not
`Sync`, because the wrapper itself doesn't allow mutable sharing;
that's what `SharedSync[Mutex[T]]` is for).

**Layout:** a heap allocation prefixed with an 8-byte atomic
refcount header, followed by the payload (i64 by ABI). `clone()`
calls `__atomic_fetch_add` on the header. Drop calls
`__atomic_fetch_sub`; on zero, the payload is dropped (calling T's
drop if T implements Drop) and the allocation is `free`'d.

## B10 — Cross-thread sharing pattern

The canonical pattern (covered by the spawn_join e2e case):

```rx
use std.sync.{Thread, Mutex, SharedSync}

counter = SharedSync.new(Mutex.new(0))
handles = []
10.times do
  c = counter.clone
  handles.push(Thread.spawn do
    g = c.lock!
    g.deref_var.set(g.deref + 1)
  end)
end
handles.each { |h| h.join }
assert_eq(counter.lock!.deref, 10)
```

## B11 — `PoisonError` / `ThreadPanic`

| Type            | Carries                            |
|-----------------|------------------------------------|
| `PoisonError`   | `message: String`                  |
| `ThreadPanic`   | `message: String, thread_id: ThreadId` |

Both implement `Display` and `Error` (mixin). Constructed by the
runtime; user code rarely instantiates directly.

## B12 — `Send` / `Sync` as manual marker mixins

`Send` and `Sync` are registered as built-in mixins (alongside
`Display`, `Clone`, `Drop`, etc.). They are **markers only** — no
required methods. Users opt in by writing `include Send` (and/or
`include Sync`) inside a class body.

**Auto-derived for built-ins:**
- All primitive types (`Int`, `Bool`, `Float`, `Char`) auto-derive
  both `Send` and `Sync`.
- `String` auto-derives both (immutable + reference-counted).
- `Array[T]`, `Option[T]`, `Result[T, E]`, `Box[T]`, `Hash[K, V]`,
  `Set[T]` auto-derive `Send` iff all type parameters are `Send`;
  `Sync` iff all type parameters are `Sync`.
- `Mutex[T]` auto-derives `Send + Sync` iff `T: Send`.
- `SharedSync[T]` auto-derives `Send + Sync` iff `T: Send`.
- `MutexGuard[T]` does NOT auto-derive `Send` (must not cross
  thread boundaries — RAII tied to current thread).

**For user classes:** `Send`/`Sync` are NOT auto-derived. A user
class that wraps only `Send` fields is still not `Send` until the
user adds `include Send`. This is the price of "manual markers" —
chosen explicitly to defer the auto-mixin engine to a follow-up.

## B13 — Borrow-check rule: thread-crossing closures (E1100)

`Thread.spawn` is special-cased in the borrow checker. For each
captured variable in the closure:
1. Compute the type's mixin set.
2. If `Send` is absent, emit E1100 at the capture site with the
   variable name and the inferred type.

The check fires only at the `Thread.spawn` call site, not on
arbitrary closures.

## B14 — Negative test: non-Send capture rejected

**Given** a user class `Foo` without `include Send`
**When** `Thread.spawn do || foo.bar end` is compiled
**Then** E1100 is emitted at the `foo` capture with message
`captured value of type Foo does not implement Send`.

## B15 — Negative test: `Mutex` with non-Send T (E1101)

**Given** a user class `Foo` without `include Send`
**When** `Mutex.new(Foo.new)` is compiled
**Then** E1101 is emitted at the `Mutex.new` call site.

## B16 — Negative test: `SharedSync` with non-Send T (E1102)

Same shape as B15 but with `SharedSync.new`. Diagnostic E1102.

## B17 — Stress: 100 threads × Mutex counter

**Given** 100 threads each incrementing a `SharedSync[Mutex[Int]]`
counter 1000 times
**Then** the final value is exactly 100_000 (no lost updates).

## B18 — Bench gate: 100k lock/unlock cycles < 100ms

Single-threaded `Mutex.lock!()` followed by drop, repeated 100_000
times, completes in under 100ms on the CI baseline. Failure of this
bench fails the spec.

---

## Pin tests

| Behaviour | Test fn                                              | File                       |
|-----------|------------------------------------------------------|----------------------------|
| B1        | `std_sync_concurrency_surface_typechecks_cleanly`    | `std_use_resolution.rs`    |
| B2, B3    | `std_sync_thread_sleep_and_yield_round_trip`         | `std_use_resolution.rs`    |
| B4        | `thread_current_id_round_trip`                       | `std_sync_runtime.rs`      |
| B5, B6    | `thread_spawn_join_round_trip`                       | `std_sync_runtime.rs`      |
| B7, B8    | `mutex_lock_unlock_round_trip`                       | `std_sync_runtime.rs`      |
| B9        | `sharedsync_clone_drop_refcount`                     | `std_sync_runtime.rs`      |
| B10       | e2e `cases/540_spawn_join_round_trip.rx`            | release-e2e                |
| B11       | `poison_error_and_thread_panic_display`              | `std_sync_runtime.rs`      |
| B12       | `send_sync_marker_mixins_register_and_resolve`       | `concurrency_markers.rs`   |
| B13, B14  | `thread_spawn_rejects_non_send_capture_e1100`        | `concurrency_negative.rs`  |
| B15       | `mutex_new_rejects_non_send_t_e1101`                 | `concurrency_negative.rs`  |
| B16       | `sharedsync_new_rejects_non_send_t_e1102`            | `concurrency_negative.rs`  |
| B17       | e2e `cases/541_mutex_counter_stress.rx`             | release-e2e                |
| B18       | bench `tests/benches/concurrency_lock_throughput.rs` | bench                      |

---

## Out of scope (deferred to follow-ups)

- **Auto-derived `Send`/`Sync` for user classes.** Requires the
  auto-mixin engine (field-set walking). Tracked as a follow-up
  prompt; manual `include Send` covers the gap.
- **Panic unwinding.** Currently `ruxen_panic` aborts. Once unwind
  support lands, `JoinHandle.join` must catch the unwind and return
  `Err(ThreadPanic)`; for now, a panicked thread aborts the whole
  process, so B6's Err arm is reachable only via an explicit
  `Thread.panic_for_testing(msg)` test hook.
- **`thread_local!` storage.** Separate prompt.
- **`Barrier`, `Semaphore`, `CondVar`.** Out of MVP per the
  primitive-scope decision.
- **Mutex/SharedSync interior storing > 8 bytes inline.** Same ABI
  limit as `Vec[T]`: users wrap big payloads in `Box[T]` first.
