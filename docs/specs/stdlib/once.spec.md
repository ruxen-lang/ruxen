# Spec — `std.sync.Once` / `OnceLock`

**Source docs:**
[docs/requirements/tier1_02_concurrency.md](../../requirements/tier1_02_concurrency.md),
[docs/prompts/v1/14_phase4_concurrency.md](../../prompts/v1/14_phase4_concurrency.md).

**Status:** new in the multithreading round. One-shot
synchronisation primitive for lazy initialisation.

Backed by `pthread_once_t` for `Once`, with an extra payload slot
for `OnceLock[T]`. Avoids the boilerplate of "static AtomicBool
guard + Mutex initialiser" by hiding the state machine.

---

## B1 — Surface types

| Type                | Role                                              |
|---------------------|---------------------------------------------------|
| `Once`              | One-shot side-effect guard                        |
| `OnceLock[T: Send]` | One-shot lazy-initialised value                   |

## B2 — `Once` API

| Call                              | Returns       |
|-----------------------------------|---------------|
| `Once.new()`                      | `Once`        |
| `o.call_once(closure)`            | `()`          |
| `o.is_completed()`                | `Bool`        |

**Given** an `Once` and N threads concurrently calling
`o.call_once(do || side_effect end)`
**Then** the closure executes exactly once; every other caller
blocks until the first call completes, then returns immediately.
- Closure panic does NOT mark the Once as completed (subsequent
  callers retry; same as `std::sync::Once` in Rust).

## B3 — `OnceLock[T]` API

| Call                              | Returns                  |
|-----------------------------------|--------------------------|
| `OnceLock.new()`                  | `OnceLock[T]`            |
| `ol.set(value: T)`                | `Result[nil, T]`          |
| `ol.get()`                        | `Option[&T]`             |
| `ol.get_or_init(closure)`         | `&T`                     |

- `set` returns `Err(value)` if the cell was already populated,
  giving the caller their value back instead of dropping it.
- `get` is non-blocking; returns `None` if uninitialised.
- `get_or_init` blocks until initialisation completes (closure runs
  exactly once across all threads, same discipline as `Once`).

`OnceLock[T]` requires `T: Send`.

## B4 — Single-thread round-trip

```rx
ol = OnceLock[Int].new()
assert_eq(ol.get, None)
ol.set(42).ok!
assert_eq(ol.get, Some(42))
assert(ol.set(99).is_err)   # already populated
```

## B5 — Multi-thread `get_or_init` runs closure once

**Given** 16 threads each calling `ol.get_or_init(do || expensive end)`
where `expensive` increments a shared atomic counter and returns 7
**Then**
- The counter is exactly 1 (the closure ran once).
- Every thread observes `&7` from its `get_or_init` return.

## B6 — Send-bound enforced (E1101)

`OnceLock[Foo]` where `Foo: !Send` emits E1101.

---

## Pin tests

| Behaviour | Test fn                                          | File                       |
|-----------|--------------------------------------------------|----------------------------|
| B1, B2    | `once_call_once_round_trip`                      | `std_sync_runtime.rs`      |
| B3, B4    | `oncelock_get_set_round_trip`                    | `std_sync_runtime.rs`      |
| B5        | e2e `cases/547_oncelock_get_or_init_stress.rx`  | release-e2e                |
| B6        | `oncelock_rejects_non_send_t_e1101`              | `concurrency_negative.rs`  |

---

## Out of scope

- `LazyLock[T, F]` (`Lazy<T>` with the initialiser baked in at
  type level) — needs HRTBs from #08.
- Reset / clear API. The whole point is one-shot; reset is a foot-gun.
