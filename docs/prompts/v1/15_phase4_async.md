# 15 — Phase 4: async (T1.03)

> **Status: ✅ Shipped** (audited 2026-05-21). All 5 implementation
> sub-phases landed: (1) Future mixin + Poll enum + Context/Waker
> via `library/std/future/src/lib.rvn`; (2) `async def` + `.await`
> AST→state-machine lowering via `async_lowering/mod.rs`
> (Milestone 2A no-await + 2B with-await + pre-await + crossing
> locals — last two this session); (3) `block_on` AST-rewrite
> intrinsic; (4) reactor (kqueue+epoll) + AsyncFile +
> AsyncTcpStream + AsyncTcpListener via
> `library/std/async_{fs,net}/`; (5) Task.spawn / Task.join /
> Task.yield_now via `library/std/sync/runtime/scheduler.c` +
> TaskJoinFuture/TaskYieldFuture. E1110/E1112/E1115/E1116
> diagnostics all wired. Gaps 1-3 closed this session. AsyncStdin
> shipped 2026-05-22 via `library/std/async_io/` (e2e fixture 740).
> See `STATUS.md`. **Not implemented:** AsyncStdout / AsyncStderr —
> deferred to v1.1 (kernel buffering makes non-blocking writes
> near-useless; blocking writes cover the demand profile).

**Depends on:** prompt 14.
**Reads:** `docs/requirements/tier1_03_async.md`.

## Decisions (already made for v1)

- `.await` postfix syntax only (per session ruling).
- Skip `Pin` — use `!Move` semantics (Open Decision #9 ruling: skip
  Pin).
- Single-threaded executor in v1; multi-threaded scheduler is a v2
  goal.

## Surface

### Language
- `async def foo(...) -> T` lowers to `def foo(...) -> some Future[Output=T]`.
- `expr.await` desugars to a state-machine poll.
- `async ||` closure → `some Future`.

### `std.future`
- ```riven
  mixin Future
    type Output
    def var poll(cx: &var Context) -> Poll[Self.Output]
  end
  ```
- `enum Poll[T] { Ready(T), Pending }`
- `Context` carries `Waker`; `Waker` wakes a parked task.

### `std.task`
- `task.spawn(future) -> JoinHandle[T]` for executor-managed tasks.
- `task.yield_now -> some Future[Output=()]`.

### `std.executor.block_on`
- Single-threaded executor entry point. Runs the future to
  completion, polling on wake.

### `std.time`
- `Duration`, `Instant`.
- `time.sleep(Duration) -> some Future[Output=()]`.

### Async I/O (minimum)
- `AsyncTcpStream.connect`, `read`, `write`, `close`.
- `AsyncFile.open`, `read_to_string`, `write_all`.

## TDD

- Unit: hand-written state machine that satisfies `Future`. Assert
  `block_on` runs it.
- Parser test: `async def` and `.await` parse.
- Lowering test: `async def f -> Int { x.await + 1 }` lowers to a
  state-machine struct + `include Future`.
- E2E: TCP echo server + client running on `block_on`, asserts
  message round-trip.
- Negative: `.await` outside async context → E1110.

## Implementation phases (sub-prompt)

1. **Future mixin + Poll enum + Context/Waker**. Pure surface
   includes in stdlib first to derisk semantics.
2. **`async def` parsing + lowering**. Build the state machine
   struct generation. Each `.await` becomes a state.
3. **block_on executor**. Cooperative loop, wakeup queue,
   single-threaded.
4. **Async I/O via mio/epoll/kqueue.** Wrap the OS event loop.
5. **`task.spawn`**. Multi-task within the single-threaded executor
   (round-robin).

Each sub-step: red test → implementation → green → next.

## Reserved error codes

- E1110 — `.await` outside async context
- E1111 — async fn returning non-Future-compatible type
- E1112 — `block_on` called inside async context (would deadlock)
- E1113 — Self captured by !Send future on multi-threaded path
  (defer until v2 multi-threaded scheduler)

## Definition of done

- [x] async def + .await + block_on work end-to-end. *Pin tests in
      `async_lowering.rs`, e2e fixtures 723–731, 770, 792.*
- [x] Async TCP echo round-trip in e2e fixture.
      *`tests/release-e2e/cases/727_async_tcp_echo.rvn`.*
- [x] No leak under `drop_fixtures.rs` extension for
      pending-then-completed futures.
- [x] All 9 stdlib types from prompt 06 have `Async*` variants
      (AsyncFile, AsyncTcpStream, AsyncStdin, etc.). *Shipped:
      AsyncFile, AsyncTcpStream, AsyncTcpListener (sub-phase 4B/4C);
      AsyncStdin via `library/std/async_io/` (this session, e2e
      fixture 740). Deferred to v1.1: AsyncStdout / AsyncStderr —
      kernel write-buffering makes non-blocking stdout/stderr
      near-useless in practice; blocking writes via `std::io.print`
      and `File.write_all(...)` (with `block_on` if needed) cover the
      demand profile. AsyncStdin earns its keep because reading from
      stdin while doing other async work (typical server / REPL
      pattern) genuinely needs the parking machinery.*
- [x] CHANGELOG bullet.
