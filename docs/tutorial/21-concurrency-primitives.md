# Concurrency Primitives

Sometimes you want your program to do more than one thing at the same time — a worker pool computing in parallel, a background thread writing logs, a counter being bumped from many places at once. Ruxen's `std.sync` module gives you the building blocks: OS threads, mutexes (shared locks), shared reference-counted pointers, and atomic integers. This chapter walks through each one with a working example, then combines them into a small parallel counter. Async / await is a separate feature for non-blocking I/O — see [Chapter 24](24-async.md) for that.

A few quick definitions before we start:

- **Thread** — an independent path of execution. Multiple threads run concurrently and may run on different CPU cores.
- **Mutex** (short for "mutual exclusion") — a lock around some data. Only one thread can hold the lock at a time, so the data inside is safe to read or change.
- **Atomic** — an integer whose updates are indivisible from another thread's view. Useful for counters and flags without taking a lock.

---

## 1. Your first thread

Save as `thread_demo.rx`:

```ruxen
use std.sync.Thread
use std.sync.JoinHandle

def main
  let handle = Thread.spawn_raw({ || 42 })
  let result = JoinHandle.join_raw(handle)
  puts "child returned #{result}"
end
```

Run it:

```bash
ruxen run thread_demo.rx
```

Output:

```
child returned 42
```

What just happened: `Thread.spawn_raw` started a fresh OS thread, ran the closure on it (which returned `42`), and gave us a `handle`. `JoinHandle.join_raw(handle)` then blocked the main thread until the child finished, and gave us the child's return value.

The spawned closure must return `Int` — that integer is your channel back. For richer data, share state via the mutex pattern in section 4.

## 2. Cooperating with the scheduler

Two helpers that are useful in any program:

```ruxen
use std.sync.Thread
use std.time.Duration

def main
  Thread.sleep(Duration.from_millis(50))   # block for 50 ms
  Thread.yield_now                          # let other threads run
  puts "ok"
end
```

- `Thread.sleep(d: &Duration)` — block the current thread for the given duration.
- `Thread.yield_now` — give the scheduler a chance to run something else.
- `Thread.current_id -> Int` — the current thread's id (non-zero for any live thread).

Both `sleep` and `yield_now` are safe to call from anywhere and useful in retry loops or polite backoff.

## 3. Spawning a function instead of an inline closure

Big closure bodies are hard to read. Pull the work into a named function and wrap it in a one-line closure at the spawn site:

```ruxen
use std.sync.Thread
use std.sync.JoinHandle
use std.time.Duration

def worker -> Int
  Thread.sleep(Duration.from_millis(10))
  100
end

def main
  let h = Thread.spawn_raw({ || worker() })
  puts "got #{JoinHandle.join_raw(h)}"
end
```

The bare identifier `worker` parses as a function *reference*, so the parens-call form `worker()` is required inside the closure.

## 4. `Mutex[T]` — guarded mutable state

A `Mutex[T]` wraps a value of type `T` behind a lock. Only one thread can hold the lock at a time — anyone else trying to lock blocks until the lock is released.

```ruxen
use std.sync.Mutex

def main
  let m = Mutex.new(10)        # m: Mutex[Int]
  let g = m.lock_raw           # g: MutexGuard[Int], lock acquired
  let v = g.get                # v: Int = 10
  g.set(v + 1)                 # write through the guard
  # g goes out of scope here -> lock is released automatically
end
```

The **guard** (`g` above) represents your hold on the lock. While it's alive, you have exclusive access. When the guard's binding goes out of scope, the lock is released. You don't call `unlock` by hand.

A guard cannot be moved to another thread — it must be dropped on the same thread that took the lock.

Other methods on `Mutex[T]`:

- `try_lock_raw -> MutexGuard[T]` — non-blocking; returns the guard immediately if free.
- `is_poisoned -> Int` — non-zero if a previous holder panicked while holding the lock.
- `clear_poison -> nil` — clears the poison flag.
- `into_inner_raw -> T` — consumes the mutex and returns the inner value.

## 5. `SharedSync[T]` — sharing a value across threads

A `Mutex[T]` lives in one place. If you want multiple threads to all hold a *reference* to the same mutex, you need a way to share ownership safely. `SharedSync[T]` is Ruxen's shared, reference-counted handle (similar to Rust's `Arc`).

Each `.clone` bumps an atomic count; each drop decrements it; when the last reference drops, the value is freed.

```ruxen
use std.sync.SharedSync

def main
  let s  = SharedSync.new(100)   # count = 1
  let s2 = s.clone                # count = 2
  puts "s  = #{s.get}"
  puts "s2 = #{s2.get}"
  puts "count = #{s.strong_count}"
end
```

Output:

```
s  = 100
s2 = 100
count = 2
```

The combination you'll use most is `SharedSync[Mutex[T]]` — the outer `SharedSync` is what lets each thread own a handle; the inner `Mutex` is what makes the data safe to mutate.

## 6. Atomic integers

For simple counters and flags, taking a full mutex is overkill. Use an atomic integer instead:

```ruxen
use std.sync.AtomicI64

def main
  let a   = AtomicI64.new(0)
  let old = a.fetch_add(5)       # adds 5, returns previous value (0)
  let cur = a.load                # reads current value (5)
  puts "old=#{old} cur=#{cur}"
end
```

`AtomicI64` surface:

- `new(initial: Int) -> AtomicI64`
- `load -> Int`
- `store(value: Int) -> nil`
- `fetch_add(delta: Int) -> Int` — returns the **previous** value
- `fetch_sub(delta: Int) -> Int`
- `compare_and_swap(current: Int, new_val: Int) -> Int`

`AtomicBool` and `AtomicUSize` follow the same shape with `Bool` and `USize` payloads.

All atomic operations use sequentially consistent ordering — the strongest, easiest-to-reason-about kind. There is no relaxed / acquire / release knob in v1.

## 7. Putting it together — a parallel counter

This program spawns two threads, each of which bumps a shared counter 1000 times, and prints the final value:

```ruxen
use std.sync.{Thread, Mutex, SharedSync, JoinHandle}

def main
  let shared = SharedSync.new(Mutex.new(0))

  let s1 = shared.clone
  let h1 = Thread.spawn_raw({ ||
    var i = 0
    while i < 1_000
      let g = s1.get.lock_raw
      g.set(g.get + 1)
      i += 1
    end
    0
  })

  let s2 = shared.clone
  let h2 = Thread.spawn_raw({ ||
    var i = 0
    while i < 1_000
      let g = s2.get.lock_raw
      g.set(g.get + 1)
      i += 1
    end
    0
  })

  let _ = JoinHandle.join_raw(h1)
  let _ = JoinHandle.join_raw(h2)

  let g = shared.get.lock_raw
  puts "final = #{g.get}"   # final = 2000
end
```

The clone-per-thread pattern is the idiomatic shape. Each thread holds its own `SharedSync` handle (`s1`, `s2`); each one dereferences it (`s.get`) to reach the inner `Mutex`, takes the lock, bumps the integer, and releases.

> **Try it:** remove the `Mutex` and use a plain `SharedSync[Int]` — does the counter still reach 2000? (Spoiler: no, and Ruxen will not let you compile this without a mutex or an atomic to guard the data.)

## 8. Common mistakes

- **Holding a guard for too long.** A `MutexGuard` keeps the lock until it's dropped. If you bind it to a long-lived `let`, every other thread blocks waiting. Take the lock, do the smallest amount of work, let the guard drop.
- **Sleeping with `Thread.sleep` from async code.** That blocks the OS thread the whole event loop is running on. Use `Async.sleep` ([Chapter 24](24-async.md)) when inside an `async def`.
- **Forgetting to `.clone` the `SharedSync`.** If you move the only handle into a spawned closure, no one else can use it. Each thread needs its own clone.
- **Using `AtomicI64` for non-trivial state.** Atomics are for single integers. For "shared list" or "shared map" the answer is `SharedSync[Mutex[T]]`.

---

## Recap

- `Thread.spawn_raw({ || ... })` starts a thread; `JoinHandle.join_raw(h)` waits for its result.
- `Mutex[T]` wraps a value behind a lock; `.lock_raw` returns a guard you read / write through.
- `SharedSync[T]` is a shared, reference-counted handle. Clone it per thread.
- `SharedSync[Mutex[T]]` is the idiomatic shape for mutable shared state.
- `AtomicI64`, `AtomicBool`, `AtomicUSize` for lock-free counters and flags.

**Next:** [Chapter 22 — In-Body Directives](22-attributes.md) for the metadata directives you attach to types and methods.
