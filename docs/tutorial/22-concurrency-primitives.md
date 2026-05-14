# Concurrency Primitives (typeck-only preview)

> **Status:** Type surface ships now.  Runtime support is **`Thread.sleep`
> + `Thread.yield_now` only**.  `Mutex`, `SharedSync`, `JoinHandle`, and
> `Thread.spawn` typecheck but currently panic with "not implemented"
> at runtime.  Full implementations land in Phase 4.
>
> **See also:** [Spec — std.sync](../specs/stdlib/sync.spec.md) for
> the full v1 typeck surface + pin tests.

`std.sync` is Riven's concurrency module.  The user-facing surface
mirrors Rust closely, but v1 ships the types in two waves:

1. **Now (Phase 1-3):** typeck contract + the two utility helpers
   (`Thread.sleep`, `Thread.yield_now`).  Lets you write the shape of
   your concurrent code today.
2. **Later (Phase 4):** full runtime — `pthread_create` for
   `Thread.spawn`, atomic refcounts for `SharedSync`, `pthread_mutex_*` for
   `Mutex`.

This chapter shows what's typeable today, with explicit markers for
"types ✓ / runtime ✗".

---

## 1. The two helpers that work today

```riven
use std.sync.Thread

def main
  Thread.sleep(1_000_000)        # nanoseconds
  Thread.yield_now()
  puts "ok"
end
```

- `Thread.sleep(ns: Int)` → POSIX `nanosleep(3)`.  Returns `()`.
- `Thread.yield_now()` → POSIX `sched_yield(3)`.  Returns `()`.

Both are real, runnable, and pin-tested by
`std_sync_thread_sleep_and_yield_round_trip`.

---

## 2. Mutex (typeck only)

```riven
use std.sync.{Mutex, MutexGuard, PoisonError}

def main
  let m: Mutex[Int] = Mutex.new(0)               # types ✓ runtime ✗
  let g: MutexGuard[Int] = m.lock!()             # types ✓ runtime ✗
  puts "value = #{g.deref()}"
end
```

The types resolve and the methods you'd expect exist:

| Method                | Returns                                | Status      |
|-----------------------|----------------------------------------|-------------|
| `Mutex.new(v)`        | `Mutex[T]`                             | types ✓ runtime ✗ |
| `m.lock()`            | `Result[MutexGuard[T], PoisonError]`   | types ✓ runtime ✗ |
| `m.lock!()`           | `MutexGuard[T]` (panics on poison)     | types ✓ runtime ✗ |
| `m.try_lock()`        | `Option[MutexGuard[T]]`                | types ✓ runtime ✗ |
| `m.into_inner()`      | `Result[T, PoisonError]`               | types ✓ runtime ✗ |
| `g.deref()`           | `&T`                                   | types ✓ runtime ✗ |
| `g.deref_mut()`       | `&mut T`                               | types ✓ runtime ✗ |

You can write code against this surface today — the typechecker will
catch shape errors and the program will compile.  Running the binary
hits a "Mutex.lock not implemented" panic.  When Phase 4 lands, the
same source compiles unchanged with a working runtime.

---

## 3. SharedSync (typeck only)

`SharedSync[T]` is Riven's atomically reference-counted, thread-safe
shared pointer. Cloning a `SharedSync` bumps the atomic refcount;
when the last clone drops, the value is freed.

```riven
use std.sync.SharedSync

def main
  let shared: SharedSync[Int] = SharedSync.new(42)   # types ✓ runtime ✗
  let snd: SharedSync[Int] = shared.clone()
  puts "count = #{shared.strong_count()}"
end
```

| Method                       | Returns          | Status            |
|------------------------------|------------------|-------------------|
| `SharedSync.new(v)`          | `SharedSync[T]`  | types ✓ runtime ✗ |
| `shared.clone()`             | `SharedSync[T]`  | types ✓ runtime ✗ |
| `shared.strong_count()`      | `USize`          | types ✓ runtime ✗ |
| `shared.weak_count()`        | `USize`          | types ✓ runtime ✗ |
| `shared.deref()`             | `&T`             | types ✓ runtime ✗ |

---

## 4. Spawning threads (typeck only)

```riven
use std.sync.{Thread, JoinHandle, ThreadPanic}

def main
  let h: JoinHandle[Int] = Thread.spawn({ || 42 })  # types ✓ runtime ✗
  let result: Result[Int, ThreadPanic] = h.join()
  match result
    Ok(v)  -> puts "got #{v}"
    Err(_) -> puts "thread panicked"
  end
end
```

Closures that return a value are typed faithfully, and the
`JoinHandle[T]` knows what `T` is.  When Phase 4 ships, this same
code will spawn a real OS thread, run the closure, and surface the
return value (or any panic) through `join`.

---

## 5. What you can do today

Even with the runtime gaps, the typeck surface is useful:

- **Design APIs that take `SharedSync[Mutex[T]]` shared state** and have
  the compiler check the types end-to-end.
- **Prototype the call-site ergonomics** of `lock!()` vs
  `lock().unwrap_or(...)` etc.  The diagnostics are the same the
  full runtime will surface.
- **Write your tests against `Thread.sleep`** so they cover the time-
  sensitive code paths even before real threads work.

What you **can't** do today:

- Run any program that actually contends a `Mutex` or spawns a
  thread.
- Benchmark concurrent code.
- Write integration tests that exercise multi-threaded behaviour.

---

## 6. When Phase 4 lands

The Phase 4 prompt is
[`docs/prompts/v1/14_phase4_concurrency.md`](../prompts/v1/14_phase4_concurrency.md).
The expected sequence:

1. **`Thread.spawn`** → `pthread_create(3)`.  Closure body becomes
   the thread function; return value lands in a heap-allocated
   slot the `JoinHandle` reads.
2. **`Mutex`** → `pthread_mutex_*`.  Poison detection by setting a
   sentinel inside `panic`'s unwind path.
3. **`SharedSync`** → bare `__atomic_fetch_add` on a 64-bit refcount
   embedded in the heap allocation.

Each piece gets its own spec section in `sync.spec.md` and its own
pin test as it ships.  Today's typeck pins guarantee the API shape
doesn't drift while the runtime is built up underneath.

---

**Next:** [Chapter 14 — Foreign Function Interface](14-ffi.md) if
you want to call C threads directly, or browse the
[spec index](../specs/README.md) to see every area with a formal
contract.
