# Spec — `Task.spawn` + `Task.yield_now` (async sub-phase 5)

**Source docs:**
[docs/requirements/tier1_03_async.md](../../requirements/tier1_03_async.md),
[docs/prompts/v1/15_phase4_async.md](../../prompts/v1/15_phase4_async.md),
[docs/specs/stdlib/executor.spec.md](executor.spec.md) (sub-phase 3 block_on),
[docs/specs/stdlib/async_io.spec.md](async_io.spec.md) (sub-phase 4 I/O).

**Status:** sub-phase 5 of the async round. Sub-phases 1–4 shipped
the surface + lowering + executor + I/O. This sub-phase wires
**multi-task scheduling** so multiple concurrent futures can be
polled by one executor on one thread. The final async piece.

After this lands, the full async-server pattern works:
```rx
async def server_loop()
  let listener = AsyncTcpListener.bind("127.0.0.1:8080").await.ok!
  loop
    let conn = listener.accept.await.ok!
    Task.spawn(handle_connection(conn))   # non-blocking
  end
end

def main
  block_on(server_loop())
end
```

---

## B1 — `Task.spawn(future) -> JoinHandle[T]`

Class-static method on `class Task` in `library/std/future/src/lib.rx`
(sibling to `class Async`). Hands the future to the executor's task
queue; returns immediately with a `JoinHandle[T]` that can later be
awaited for the spawned task's Output.

```rx
let handle = Task.spawn(some_async_fn())
# ... other work, possibly more spawns ...
let result = handle.await   # blocks until the spawned task completes
```

**Given** an executor running via `block_on`
**When** `Task.spawn(future)` is called from inside an async context
**Then** the future is added to the executor's task queue. `Task.spawn`
returns a `JoinHandle[T]` immediately (does NOT poll the future).
The executor polls the queued future as part of its round-robin loop.

## B2 — `Task.yield_now() -> some Future[Output = nil]`

Cooperative yield. Returns a future that, on first poll, registers
a wakeup-on-next-tick and returns Pending; on second poll, returns
Ready.

```rx
async def long_running()
  let mut i = 0
  while i < 1_000_000
    if i % 1000 == 0
      Task.yield_now.await    # let other tasks run
    end
    i = i + 1
  end
end
```

Without `yield_now`, a CPU-bound task starves the executor. With it,
the task voluntarily releases the executor between chunks of work.

## B3 — Executor task queue

The executor (in `library/std/future/runtime/executor.c`) gains:
- A task queue: linked list of `(future_ptr, waker_state, joinhandle_slot)` entries.
- A round-robin polling loop: walks the queue, polls each task once, removes completed tasks, parks if all pending.
- Per-task `Waker`: each task gets its own waker whose `wake()`
  re-enqueues the task into a ready-set the next poll loop drains
  before parking again.

The single-task `block_on` from sub-phase 3 becomes a special case:
the top-level future is just task #0 in the queue, with no
join-handle (`block_on` blocks the caller for it).

## B4 — `JoinHandle[T]` for tasks

Different shape from `Thread.spawn`'s JoinHandle (OS-thread handle):
in-executor task handles wrap a result slot the executor fills when
the task completes, plus a per-task waker queue for callers
awaiting the result.

```rx
class JoinHandle[T: Send]   # already exists for Thread.spawn
  # New method for in-executor tasks:
  def await(self) -> T
    # Suspends the current task until the joined task completes.
    # Synthesized into the calling async fn's state machine.
  end
end
```

Or: a separate `TaskHandle[T]` class with the same shape. (Spec
prefers reusing `JoinHandle[T]` for surface symmetry — the
JoinHandle's internal kind can branch between OS-thread vs.
in-executor task.)

## B5 — `Waker.wake` becomes real

Sub-phase 3 left `Waker.wake` / `Waker.wake_by_ref` as no-ops. After
sub-phase 5, each `Waker` is tied to a specific task; calling `wake`
moves that task from the pending pool back into the ready queue.
The executor's main loop drains the ready queue before parking on
the reactor for the next event.

`Context.test_dummy`'s waker continues to be a no-op (unit-test
scenarios don't need real wake signaling).

## B6 — `block_on` and `Task.spawn` interop

`block_on(future)` works as today — drives `future` to completion.
With `Task.spawn` available, `future` may spawn additional tasks
during its execution; the executor polls all of them.

The generated `block_on` loop pumps the task queue at the start of
each iteration and again in the `Poll.Pending` arm before it parks.
That second pump is required for tasks spawned by the root future
during the same poll: without it, the root future can enqueue a task,
return `Pending`, and then park on an unrelated reactor registration
before the new task has ever been polled. The pending arm only calls
`Thread.yield_now` when that second pump completed no tasks and the
task queue is empty.

`block_on` returns when the TOP-LEVEL future (the one passed to
block_on) returns Ready. Any still-running tasks at that point are
**dropped** (their futures' `def drop` fires, cleaning up reactor
registrations and held resources).

Open question for v1: should `block_on` wait for ALL spawned tasks
to complete (Java `ExecutorService.awaitTermination` shape) or
just the top-level one (Rust `block_on(future)` shape)? Spec
chooses the latter — top-level only. Users wanting "wait for all"
write it explicitly via collecting JoinHandles + awaiting each.

## B7 — `Task.spawn` outside async context (E1116)

`Task.spawn(future)` called from sync code (i.e., not inside a
`block_on` or an `async def`) is rejected with E1116 — there's no
executor to enqueue into. Same shape as E1112 (`block_on` inside
async).

## B8 — Single-thread guarantee

The v1 executor runs all tasks on a single OS thread. `Task.spawn`
does NOT cross thread boundaries — the spawned future runs on
whoever's calling `block_on`. This means `Send` is not required on
spawned futures (yet); only `T: Send` is required on `JoinHandle[T]`
when joining across thread boundaries (which v1 tasks don't do —
multi-threaded scheduler is v2).

## B9 — E2E: concurrent TCP echo server

The 4C echo fixture (`727_async_tcp_echo.rx`) currently
thread-bridges (one thread per side, two `block_on` calls). With
sub-phase 5, the echo server can be one process, one `block_on`,
two `Task.spawn` calls:

```rx
async def server_loop()
  let listener = AsyncTcpListener.bind(addr).await.ok!
  let conn = listener.accept.await.ok!
  # Echo loop runs as a task; main keeps accepting (in a real server).
  Task.spawn(echo_handler(conn))
end

async def client_flow(addr: &String)
  let stream = AsyncTcpStream.connect(addr).await.ok!
  ...
end

def main
  let addr = "127.0.0.1:9001"
  block_on(do
    let server = Task.spawn(server_loop(addr))
    let client = Task.spawn(client_flow(addr))
    server.await
    client.await
  end)
end
```

E2E fixture `728_async_tcp_echo_single_block_on.rx` lands as part
of this sub-phase. Asserts the round-trip in one process, one
`block_on`.

## B10 — Drop semantics

When `block_on` returns, every task still in the queue has its
future dropped. Drop fires `def drop` on each — releasing reactor
registrations, closing fds, etc. (Per the 4B/4C drop discipline.)

If a task panics, the executor catches the panic (when unwind
support lands; v1 = abort), marks the JoinHandle as panicked, and
continues polling other tasks. v1: panic = process abort, same as
`Thread.spawn` semantics.

---

## Pin tests

| Behaviour | Test fn                                                | File                          |
|-----------|--------------------------------------------------------|-------------------------------|
| B1        | `task_spawn_returns_join_handle_without_polling`       | `tests/task_scheduler.rs`     |
| B2        | `task_yield_now_returns_pending_then_ready`            | `tests/task_scheduler.rs`     |
| B3        | `executor_round_robin_polls_all_live_tasks`            | `tests/task_scheduler.rs`     |
| B4        | `join_handle_await_blocks_until_task_completes`        | `tests/task_scheduler.rs`     |
| B5        | `waker_wake_re_enqueues_task`                          | `tests/task_scheduler.rs`     |
| B6        | `block_on_drops_remaining_tasks_on_top_level_complete` | `tests/task_scheduler.rs`     |
| B7        | `task_spawn_outside_async_rejected_e1116`              | `tests/async_negative.rs`     |
| B9        | e2e `cases/728_async_tcp_echo_single_block_on.rx`     | release-e2e                   |
| B10       | `dropped_task_releases_reactor_registrations`          | `tests/task_scheduler.rs`     |

---

## Out of scope (v2)

- **Multi-threaded work-stealing executor.** v1 single-threaded.
- **Task priority / fairness policies.** v1 round-robin only.
- **Task local storage** (`task_local!`). Separate prompt.
- **Cancellation** (`JoinHandle.cancel`). v2 — needs cooperative drop discipline.
- **`select!` macro across multiple futures.** v2.

## Reserved error codes (new)

- **E1116** — `Task.spawn` called outside an async / `block_on` context.
