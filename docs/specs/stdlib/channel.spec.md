# Spec — `std.sync.mpsc` (Channel)

**Source docs:**
[docs/requirements/tier1_02_concurrency.md](../../requirements/tier1_02_concurrency.md),
[docs/prompts/v1/14_phase4_concurrency.md](../../prompts/v1/14_phase4_concurrency.md).

**Status:** new in the multithreading round. Lives under
`std.sync.mpsc` (multi-producer, single-consumer). Builds on
[sync.spec.md](sync.spec.md) — channels require `Send` on the
element type at compile time (E1101 extended).

A `Channel[T]` is a thread-safe queue with `O(1)` amortised
send/recv. The MVP cut ships **unbounded** channels only; bounded
channels and `select!` are deferred.

---

## B1 — Surface types

| Type                        | Role                                             |
|-----------------------------|--------------------------------------------------|
| `Sender[T: Send]`           | Producer half (cloneable; multi-producer)        |
| `Receiver[T: Send]`         | Consumer half (single-consumer; not cloneable)   |
| `SendError`                 | Returned by `send` when receiver dropped         |
| `RecvError`                 | Returned by `recv` when all senders dropped      |

`channel[T]() -> (Sender[T], Receiver[T])` is a top-level
constructor under `std.sync.mpsc`.

## B2 — `channel[T]() -> (Sender[T], Receiver[T])`

**Given** `let (tx, rx) = channel[Int]()`
**Then** `tx` and `rx` are linked. Sending on `tx` makes the value
available on `rx`. Both halves are independently `Send`.

## B3 — `Sender.send(value: T) -> Result[(), SendError]`

**Given** an open channel and `tx.send(42)`
**Then** the value is enqueued and `Ok(())` is returned.
**When** the matching `Receiver` has been dropped
**Then** `Err(SendError)` is returned (no panic).

## B4 — `Sender.clone() -> Sender[T]`

**Given** `let tx2 = tx.clone()`
**Then** both `tx` and `tx2` reference the same channel. The
channel stays open as long as at least one sender is alive.

## B5 — `Receiver.recv() -> Result[T, RecvError]`

**Given** an open channel with a value available
**Then** `rx.recv()` returns `Ok(value)`.
**When** the queue is empty
**Then** the call blocks until either a value arrives (returns
`Ok`) or every sender has been dropped (returns `Err(RecvError)`).

## B6 — `Receiver.try_recv() -> Option[T]`

**Given** an open channel
**Then** `rx.try_recv()` returns `Some(value)` if one is available,
`None` otherwise (non-blocking, no error path — channel-closed is
not surfaced).

## B7 — Drop semantics: receiver close → sender error

**Given** `tx` alive, `rx` dropped
**When** `tx.send(...)` is called
**Then** `Err(SendError)` (no enqueue, no leak — value's drop fires).

## B8 — Drop semantics: all senders dropped → recv error

**Given** the queue is drained and every `Sender` is dropped
**When** `rx.recv()` is called
**Then** `Err(RecvError)`. If the queue has buffered values, those
are drained first (Ok) and only the next call after drainage
returns Err.

## B9 — Send-bound enforced (E1101)

**Given** a user class `Foo` without `include Send`
**When** `channel[Foo]()` is compiled
**Then** E1101 is emitted at the `channel` call site.

## B10 — Ping-pong e2e

**Given** two threads, one calling `tx.send(i)` for i in 0..1000
and the other calling `rx.recv()` in a loop summing the results
**Then** the receiver sums to 499_500 and exits cleanly when the
sender drops `tx`.

---

## Implementation notes (informative)

- Channel state: heap-allocated control block `{ mutex, condvar,
  queue, sender_count, closed_flag }` shared via atomic refcount
  (same shape as `SharedSync` payload).
- `Sender` clone increments `sender_count` atomically; Drop
  decrements and, on zero, signals the condvar (so blocked
  receivers wake to find the channel drained-and-closed).
- `Receiver` Drop sets `closed_flag` so any pending `send` errors
  cleanly.
- Queue: linked list of i64 payload slots. No bounded variant in
  this round — `bounded(n)` is a follow-up.
- Each i64 element carries T per the same ABI rule as Mutex
  (inline if ≤ 8 bytes, otherwise pointer to heap T).

---

## Pin tests

| Behaviour | Test fn                                          | File                         |
|-----------|--------------------------------------------------|------------------------------|
| B1, B2    | `channel_constructor_pair_resolves`              | `std_sync_runtime.rs`        |
| B3, B5    | `channel_send_recv_round_trip`                   | `std_sync_runtime.rs`        |
| B4        | `channel_sender_clone_multi_producer`            | `std_sync_runtime.rs`        |
| B6        | `channel_try_recv_nonblocking`                   | `std_sync_runtime.rs`        |
| B7        | `channel_receiver_dropped_send_errs`             | `std_sync_runtime.rs`        |
| B8        | `channel_all_senders_dropped_recv_errs`          | `std_sync_runtime.rs`        |
| B9        | `channel_rejects_non_send_t_e1101`               | `concurrency_negative.rs`    |
| B10       | e2e `cases/542_channel_pingpong.rx`             | release-e2e                  |

---

## Out of scope

- Bounded channels (`bounded(n)`) — follow-up prompt.
- `select!` macro across multiple receivers.
- Broadcast channels (single-producer multi-consumer).
- Sync-channel zero-capacity rendezvous.
