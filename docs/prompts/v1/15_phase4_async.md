# 15 — Phase 4: async (T1.03)

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

- [ ] async def + .await + block_on work end-to-end.
- [ ] Async TCP echo round-trip in e2e fixture.
- [ ] No leak under `drop_fixtures.rs` extension for
      pending-then-completed futures.
- [ ] All 9 stdlib types from prompt 06 have `Async*` variants
      (AsyncFile, AsyncTcpStream, AsyncStdin, etc.).
- [ ] CHANGELOG bullet.
