# Spec — `std.sync.RwLock`

**Source docs:**
[docs/requirements/tier1_02_concurrency.md](../../requirements/tier1_02_concurrency.md),
[docs/prompts/v1/14_phase4_concurrency.md](../../prompts/v1/14_phase4_concurrency.md).

**Status:** new in the multithreading round. Reader-writer lock for
read-heavy workloads where `Mutex` would serialise unnecessarily.

Same shape as `Mutex[T]` but with multiple-reader / single-writer
semantics. Backed by `pthread_rwlock_t`.

---

## B1 — Surface types

| Type                | Role                                             |
|---------------------|--------------------------------------------------|
| `RwLock[T]`         | Reader-writer lock wrapper (requires `T: Send`)  |
| `ReadGuard[T]`      | RAII guard from `read()` — many can coexist      |
| `WriteGuard[T]`     | RAII guard from `write()` — exclusive            |

## B2 — Constructors and accessors

| Call                          | Returns                              |
|-------------------------------|--------------------------------------|
| `RwLock.new(value: T)`        | `RwLock[T]`                          |
| `rw.read()`                   | `Result[ReadGuard[T], PoisonError]`  |
| `rw.read!()`                  | `ReadGuard[T]` (panics on poison)    |
| `rw.write()`                  | `Result[WriteGuard[T], PoisonError]` |
| `rw.write!()`                 | `WriteGuard[T]` (panics on poison)   |
| `rw.try_read()`               | `Option[ReadGuard[T]]`               |
| `rw.try_write()`              | `Option[WriteGuard[T]]`              |
| `rw.into_inner()`             | `Result[T, PoisonError]`             |

## B3 — `ReadGuard` deref + Drop

| Call                          | Returns       |
|-------------------------------|---------------|
| `g.deref()`                   | `&T`          |

`ReadGuard` is Drop; scope exit calls `pthread_rwlock_unlock`. No
`deref_var` — read guards are read-only.

## B4 — `WriteGuard` deref + Drop

| Call                          | Returns       |
|-------------------------------|---------------|
| `g.deref()`                   | `&T`          |
| `g.deref_var()`               | `&var T`      |

Same Drop semantics as `ReadGuard`.

## B5 — Multiple readers concurrent

**Given** 8 threads simultaneously calling `rw.read!()`
**Then** all 8 acquire concurrently (no contention) and observe
the same value. Verified by a barrier-style test: every reader
records its acquisition timestamp, and the spread between the
fastest and slowest is bounded (< 10ms on a 100ms hold).

## B6 — Writers exclude readers

**Given** a held `WriteGuard`
**When** another thread calls `rw.read()`
**Then** the read blocks until the write guard drops.

## B7 — Send/Sync bounds

- `RwLock[T]` requires `T: Send` at construction (E1101).
- `ReadGuard[T]`, `WriteGuard[T]` do NOT auto-derive `Send` (tied
  to the acquiring thread, like `MutexGuard`).

## B8 — Poison semantics

Same as `Mutex`: a panic while holding the write lock poisons the
RwLock. Read locks do not poison (they hold no mutable invariant).

---

## Pin tests

| Behaviour | Test fn                                          | File                       |
|-----------|--------------------------------------------------|----------------------------|
| B1, B2    | `rwlock_constructors_resolve`                    | `std_sync_runtime.rs`      |
| B3, B4    | `rwlock_guards_deref_round_trip`                 | `std_sync_runtime.rs`      |
| B5        | e2e `cases/545_rwlock_multi_reader.rx`          | release-e2e                |
| B6        | e2e `cases/546_rwlock_writer_excludes.rx`       | release-e2e                |
| B7        | `rwlock_rejects_non_send_t_e1101`                | `concurrency_negative.rs`  |
| B8        | `rwlock_poison_on_writer_panic`                  | `std_sync_runtime.rs`      |

---

## Out of scope

- Read-preference vs. write-preference policy (uses pthread default).
- Upgradable read guards.
- Try-with-timeout (`try_read_for(Duration)`).
