# future/runtime — C side of the async executor, reactor and combinators

## What lives here
- `executor.c`, `scheduler.c` — Context/Waker, the per-thread task queue, `block_on`
  pump, spawn/join, Race.
- `reactor.c` — epoll/kqueue fd and timer registrations, `park_current`.
- `queue.c`, `schedule.c`, `stream.c`, `test_clock.c` — Queue, Retry, Stream
  adapter teardown, the deterministic test clock.
- `future_rt.h` — package-internal header: the weak `Drop_dynamic_drop` and
  `Future_dynamic_poll` declarations, `RUXEN_POLL_*_TAG`, and
  `ruxen_future_drop_and_free`.

## Public surface
The `ruxen_executor_*`, `ruxen_reactor_*`, `ruxen_race_*`, `ruxen_queue_*`,
`ruxen_retry_*` and `ruxen_stream_*` symbols bound from `library/std/future/src`
`lib` blocks and from async_net/async_fs/async_io C.

## Depends on
`core/runtime/runtime.h`; the compiler-synthesised `Future_dynamic_poll` /
`Drop_dynamic_drop` helpers at link time.

## Invariants & gotchas
- Include `future_rt.h` after `runtime.h`; nothing outside this dir includes it.
- `Drop_dynamic_drop` is weak: NULL means the program has no future at all.
- "Persistent" fd registration is the same OS registration as the one-shot
  entry points; persistence is the caller keeping the handle until drop.
- The async I/O futures (async_net/async_fs/async_io) park and release fd
  registrations only through `library/std/future/src/step_future.rx`, which
  also states their shared post-Ready contract (`Ready(Err(InvalidInput))`).
