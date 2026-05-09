# 14 — Phase 4: concurrency (T1.02)

**Depends on:** Phase 3 done.
**Reads:** `docs/requirements/tier1_02_concurrency.md`.

## Surface

`std::thread`:
- `Thread::spawn(closure) -> JoinHandle[T]`
- `JoinHandle::join -> Result[T, ThreadPanic]`
- `Thread::sleep(Duration)`, `Thread::yield_now`,
  `Thread::current_id`

`std::sync`:
- `Mutex[T]`, `Mutex::new(T)`, `lock -> MutexGuard[T]`
- `RwLock[T]`
- `Arc[T]`, `Arc::new(T)`, `Arc::clone(&self) -> Arc[T]`
- `Once`, `OnceLock[T]`

`std::sync::mpsc`:
- `channel[T]() -> (Sender[T], Receiver[T])`
- `Sender::send(T) -> Result[(), SendError]`
- `Receiver::recv -> Result[T, RecvError]`,
  `try_recv -> Option[T]`

`std::sync::atomic`:
- `AtomicI64`, `AtomicBool`, `AtomicUsize` with `load`, `store`,
  `compare_and_swap`, `fetch_add`, `fetch_sub`, `Ordering` enum.

Auto-traits:
- `Send`, `Sync` already declared. Fully wire negative impls and
  derivation through composite types.

## TDD

For each primitive:

- Unit test exercising the API.
- Stress test: 100 threads + Mutex<Counter> incrementing reaches
  N*100.
- Negative tests: closure capturing non-`Send` rejected with E1011.
- Channel close test: dropping last Sender returns `Err` from recv.

## Implementation

- Threads: pthread_create on Unix, CreateThread on Windows (Phase 4
  enables Windows).
- Mutex: backed by pthread_mutex_t. `MutexGuard` is a Drop type that
  releases on scope exit.
- Arc: 8-byte refcount header preceding the payload. `clone`
  increments via atomic; `Drop` decrements and frees on zero.
- Channels: bounded MPSC ring buffer; unbounded uses VecDeque
  protected by Mutex + condvar.
- `Send`/`Sync` propagate through composite types automatically
  unless `@[opt_out_send]` / `@[unsafe_impl_send]` (already in
  ClassInfo per P0.4 audit).

## Reserved error codes

- E1100 — closure captures non-Send across thread boundary
- E1101 — Mutex used with non-Send T
- E1102 — Arc::new with non-Sync T (when sharing across threads)
- E1103 — channel send error (runtime)

## Definition of done

- [ ] Every primitive has unit + stress + negative tests.
- [ ] Bench: 100k Mutex acquisitions/release < 100ms.
- [ ] Drop semantics correct: leaks proven absent under
      `drop_fixtures.rs` extensions for Mutex/Arc/Channel.
- [ ] CHANGELOG bullet.
