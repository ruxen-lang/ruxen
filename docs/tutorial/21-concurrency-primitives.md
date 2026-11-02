# Concurrency Primitives

Sometimes you want your program to do more than one thing at the same time — a worker pool computing in parallel, a background thread writing logs, a counter being bumped from many places at once. Ruxen's `std.sync` module gives you the building blocks: OS threads, mutexes (shared locks), shared reference-counted pointers, and atomic integers. This chapter walks through each one with a working example, then combines them into a small parallel counter. Async / await is a separate feature for non-blocking I/O — see [Chapter 24](24-async.md) for that.

A few quick definitions before we start:

- **Thread** — an independent path of execution. Multiple threads run concurrently and may run on different CPU cores.
- **Mutex** (short for "mutual exclusion") — a lock around some data. Only one thread can hold the lock at a time, so the data inside is safe to read or change.
- **Atomic** — an integer whose updates are indivisible from another thread's view. Useful for counters and flags without taking a lock.

The user-facing thread API is moving to Ruby's: `Thread.new { ... }.value`, `Mutex#synchronize`, `Thread::Queue` and `Thread.current[:key]` are [planned](../../TODO.md#t-thread-api). The join handles and lock guards this chapter uses today are [deprecated] as the user-facing shape and will be replaced by those methods. The sync classes are available without a `require`; the old `use std.sync.Thread` imports are [deprecated], replace them with the bare class names.

---

## 1. Your first thread

Save as `thread_demo.rx`. The Ruby form [planned](../../TODO.md#t-thread-api):

```ruby
t = Thread.new { 42 }
puts "child returned #{t.value}"
```

Works today [deprecated] (`Thread.spawn` and its join handle, replaced by `Thread.new` and `value`):

```ruby
handle = Thread.spawn { 42 }
puts "child returned #{handle.join!}"
```

Run it:

```sh
ruxen run thread_demo.rx
```

Output:

```text
child returned 42
```

What just happened: `Thread.spawn` started a fresh OS thread, ran the block on it (which returned `42`), and gave us a `handle`. `handle.join!` then blocked the main thread until the child finished, and gave us the child's return value. `handle.join` does the same but returns a `Result` that is an `Err` if the thread aborted.

The handle is typed by the block's result (`42` makes it a handle of `Int`). For richer shared data, share state via the mutex pattern in section 4. The older `Thread.spawn_raw` / `JoinHandle.join_raw` pair, which passes only an `Int` back, is [deprecated] and kept internal; real argument types for it are [planned](../../TODO.md#t-d14).

## 2. Cooperating with the scheduler

Two helpers that are useful in any program. Example [deprecated] (`Duration.from_millis`, replaced by numeric seconds such as `sleep(0.05)` [planned](../../TODO.md#t-stdlib-shapes)):

```ruby
Thread.sleep(Duration.from_millis(50))   # block for 50 ms
Thread.yield_now                         # let other threads run
puts "ok"
```

- `Thread.sleep(d)` [implemented] — block the current thread for the given duration (a `&Duration`).
- `Thread.yield_now` [implemented] — give the scheduler a chance to run something else; Ruby's `Thread.pass` is [planned](../../TODO.md#t-thread-api).
- `Thread.current_id` [implemented] — the current thread's id as an `Int` (non-zero for any live thread).

Both `sleep` and `yield_now` are safe to call from anywhere and useful in retry loops or polite backoff.

## 3. Spawning a method instead of an inline block

Big block bodies are hard to read. Pull the work into a named method and call it from a one-line block at the spawn site. Works today [deprecated] (`Thread.spawn`, `join!` and `Duration.from_millis`, replaced by `Thread.new`, `value` and `sleep(0.01)`):

```ruby
def worker -> Int
  Thread.sleep(Duration.from_millis(10))
  100
end

h = Thread.spawn { worker() }
puts "got #{h.join!}"
```

The bare identifier `worker` used as a value passes the method itself, so the call form `worker()` is required inside the block today; the bare call is [planned](../../TODO.md#t-f45).

## 4. `Mutex[T]` — guarded mutable state

A `Mutex[T]` wraps a value of type `T` behind a lock. Only one thread can hold the lock at a time — anyone else trying to lock blocks until the lock is released.

The Ruby form, `synchronize` with a block [planned](../../TODO.md#t-thread-api):

```ruby
m = Mutex.new(10)
m.synchronize { |v| v + 1 }
```

Works today [deprecated] (the lock guard, replaced by `synchronize`):

```ruby
m = Mutex.new(10)        # m: Mutex[Int]
g = m.lock!              # g: the guard, lock acquired
v = g.get                # v: Int = 10
g.set(v + 1)             # write through the guard
puts g.get               # 11
# g goes out of scope at the end of the method -> lock is released automatically
```

The **guard** (`g` above) represents your hold on the lock. While it's alive, you have exclusive access. When the guard is dropped, the lock is released. You don't call `unlock` by hand. Today a guard is dropped when the enclosing *method* returns, not at the end of an inner block, so taking a second lock on the same mutex in the same method deadlocks; take each lock in its own small method, as section 7 does.

A guard cannot be moved to another thread — it must be dropped on the same thread that took the lock.

Other methods on `Mutex[T]`:

- `lock` [deprecated] — like `lock!`, but returns a `Result` that is an `Err` when the mutex is poisoned.
- `try_lock` [deprecated] — non-blocking; returns the guard, or `nil` if the lock is taken. Ruby's `try_lock -> Bool` is [planned](../../TODO.md#t-thread-api).
- `is_poisoned` [implemented] — non-zero if a previous holder aborted while holding the lock.
- `clear_poison` [implemented] — clears the poison flag.
- `into_inner` [deprecated] — consumes the mutex and returns the inner value inside a `Result`.

## 5. `SharedSync[T]` — sharing a value across threads

A `Mutex[T]` lives in one place. If you want multiple threads to all hold a *reference* to the same mutex, you need a way to share ownership safely. `SharedSync[T]` is Ruxen's thread-safe, reference-counted handle.

Each `.clone` bumps an atomic count; each drop decrements it; when the last reference drops, the value is freed. Example [implemented]:

```ruby
s = SharedSync.new(100)   # count = 1
s2 = s.clone              # count = 2
puts "s  = #{s.get}"
puts "s2 = #{s2.get}"
puts "count = #{s.strong_count}"
```

Output:

```text
s  = 100
s2 = 100
count = 2
```

`s.get` returns a copy of the inner value, so it needs a value that can be cloned; `s.deref` borrows the inner value instead.

The combination you'll use most is `SharedSync[Mutex[T]]` — the outer `SharedSync` is what lets each thread own a handle; the inner `Mutex` is what makes the data safe to mutate.

## 6. Atomic integers

For simple counters and flags, taking a full mutex is overkill. Use an atomic integer instead. Example [implemented]:

```ruby
a = AtomicI64.new(0)
old = a.fetch_add(5)     # adds 5, returns previous value (0)
cur = a.load             # reads current value (5)
puts "old=#{old} cur=#{cur}"
```

`AtomicI64` surface, all [implemented]:

- `AtomicI64.new(initial)`
- `load` — the current value
- `store(value)`
- `fetch_add(delta)` — returns the **previous** value
- `fetch_sub(delta)`
- `compare_and_swap(current, new_val)`

`AtomicUsize` has the same methods [implemented]. `AtomicBool` stores `0` or `1` and has `load`, `store`, `fetch_and`, `fetch_or` and `compare_and_swap` [implemented].

All atomic operations use sequentially consistent ordering — the strongest, easiest-to-reason-about kind. There is no relaxed / acquire / release knob.

## 7. Putting it together — a parallel counter

This program spawns two threads, each of which bumps a shared counter 1000 times, and prints the final value. Works today [deprecated] (`Thread.spawn`, `join!` and the lock guard, replaced by `Thread.new`, `value` and `synchronize` [planned](../../TODO.md#t-thread-api)):

```ruby
def bump(s: &SharedSync[Mutex[Int]])
  g = s.deref.lock!
  g.set(g.get + 1)
end

def total(s: &SharedSync[Mutex[Int]]) -> Int
  g = s.deref.lock!
  g.get
end

shared = SharedSync.new(Mutex.new(0))

s1 = shared.clone
h1 = Thread.spawn(move {
  i = 0
  while i < 1_000
    bump(s1)
    i += 1
  end
  0
})

s2 = shared.clone
h2 = Thread.spawn(move {
  i = 0
  while i < 1_000
    bump(s2)
    i += 1
  end
  0
})

h1.join!
h2.join!

puts "final = #{total(shared)}"   # final = 2000
```

The clone-per-thread pattern is the idiomatic shape. Each thread owns its own `SharedSync` handle (`s1`, `s2`), moved into its block with `move` [implemented]; each one borrows the inner `Mutex` (`s.deref`), takes the lock, bumps the integer, and releases it when `bump` returns.

> **Try it:** remove the `Mutex` and use a plain `SharedSync[Int]` — can you still bump the counter? (Spoiler: no. `SharedSync` hands out only shared reads; changing the data needs a mutex or an atomic.)

## 8. Common mistakes

- **Holding a guard for too long.** A guard keeps the lock until it's dropped, which today is when its method returns. If you take the lock at the top of a long method, every other thread blocks waiting. Take the lock in a small method, do the smallest amount of work, let the guard drop.
- **Sleeping with `Thread.sleep` from async code.** That blocks the OS thread the whole event loop is running on. Use the async sleep ([Chapter 24](24-async.md)) when inside an `async def`.
- **Forgetting to `.clone` the `SharedSync`.** If you move the only handle into a spawned block, no one else can use it. Each thread needs its own clone.
- **Capturing a borrow in a spawned block.** Give each thread an owned `SharedSync` clone, as in section 7. Today a method call such as `Thread.spawn { r.size }` with `r = &s` still compiles and can read freed memory; the compile-time `Send` check on method calls is [planned](../../TODO.md#t-d16).
- **Using `AtomicI64` for non-trivial state.** Atomics are for single integers. For "shared list" or "shared map" the answer is `SharedSync[Mutex[T]]`.

---

## Recap

- `Thread.spawn { ... }` starts a thread and `join!` waits for its result [deprecated]; Ruby's `Thread.new { ... }.value` is [planned](../../TODO.md#t-thread-api).
- `Mutex[T]` wraps a value behind a lock; `lock!` returns a guard you read / write through [deprecated]; `synchronize` is [planned](../../TODO.md#t-thread-api).
- `SharedSync[T]` is a shared, reference-counted handle [implemented]. Clone it per thread.
- `SharedSync[Mutex[T]]` is the idiomatic shape for mutable shared state.
- `AtomicI64`, `AtomicBool`, `AtomicUsize` for lock-free counters and flags [implemented].

**Next:** [Chapter 22 — In-Body Directives](22-attributes.md) for the metadata directives you attach to types and methods.
