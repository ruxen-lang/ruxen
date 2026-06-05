# Spec — async I/O (sub-phase 4: time.sleep + AsyncFile + AsyncTcpStream)

**Source docs:**
[docs/requirements/tier1_03_async.md](../../requirements/tier1_03_async.md) §7,
[docs/prompts/v1/15_phase4_async.md](../../prompts/v1/15_phase4_async.md),
[docs/specs/stdlib/async.spec.md](async.spec.md) (sub-phase 1 surface),
[docs/specs/syntax/async_lowering.spec.md](../syntax/async_lowering.spec.md) (sub-phase 2 lowering),
[docs/specs/stdlib/executor.spec.md](executor.spec.md) (sub-phase 3 block_on).

**Status:** sub-phase 4 of the async round. Sub-phases 1–3 (commits
`b513e90`, `74c846d`, `92a5035`, `7d7898f`) shipped surface + lowering
+ a spinning block_on. This sub-phase wires the **OS event reactor**
(epoll on Linux, kqueue on macOS/BSDs) and the three minimum-viable
async I/O types: `time.sleep`, `AsyncFile`, `AsyncTcpStream`.

Three internal milestones:
- **Milestone 4A** (B1–B3 + executor park/wake): real park/wake via
  epoll/kqueue + `time.sleep(Duration)`. Smallest delta — proves the
  reactor pattern with a single Future shape (timer).
- **Milestone 4B** (B4–B6): `AsyncFile.open` / `.read_to_string` /
  `.write_all`. File I/O reuses the reactor.
- **Milestone 4C** (B7–B12): `AsyncTcpStream` connect / read / write
  / close + TCP echo e2e round-trip.

Land 4A first; commit; then 4B; then 4C.

---

## Milestone 4A — reactor + time.sleep

### B1 — `time.sleep(d: &Duration) -> some Future[Output = nil]`

```rx
use std.time.Duration
use std.time.sleep   # ← new async surface
use std.executor.block_on

def main
  block_on(sleep(Duration.from_millis(50)))
  puts "done"
end
```

**Given** the source above
**Then** the program prints `done` after sleeping ~50ms via the
async path (epoll_pwait timeout / kevent EVFILT_TIMER), NOT the
synchronous `nanosleep` path.

`time.sleep` is a free fn in `library/std/time/src/lib.rx`. Its
return type lifts to a `__TimeSleepFuture` (same AST-synthesis
pattern as `async def` — though here the function isn't `async def`,
it's a hand-written future. The lib decl declares the typed return,
the rest follows.)

Internally the timer future:
- On first poll: registers a timer with the reactor (`timerfd_create`
  + `epoll_ctl` on Linux; `kevent` with `EVFILT_TIMER` on macOS).
  Stashes the registration handle in the future's `__handle: Int`
  field. Returns `Poll.Pending`.
- On subsequent poll: checks if the registration fired. If yes,
  returns `Poll.Ready(nil)`. If no, returns `Pending` (executor will
  re-park on the reactor).

### B2 — Executor's park/wake uses the reactor

```c
// library/std/executor/runtime/executor.c
static void ruxen_executor_park(RuxenContext *ctx) {
    // Block on the reactor's wait point. The reactor is the per-
    // thread singleton that holds all registered fds/timers.
    ruxen_reactor_wait(ctx->reactor);
    // When this returns, at least one event has fired — the
    // executor's next poll iteration will see the futures that
    // were waiting on those events.
}
```

Replaces the `sched_yield`-spin from sub-phase 3. The reactor's
`wait` is `epoll_pwait` / `kevent` with a "wait until any event"
shape. No timeout — the reactor itself manages timers via timerfd /
EVFILT_TIMER.

### B3 — Reactor is a per-thread singleton

For v1 single-threaded executor, one reactor lives per thread (and
in practice only the main thread has one). The reactor is lazily
constructed on first `block_on(...)` and reused across nested
`block_on` calls (which are forbidden in async contexts but allowed
in sync contexts).

Layout: `RuxenReactor { fd: int, registered_count: int, ... }`.
The fd is the epoll/kqueue file descriptor.

---

## Milestone 4B — AsyncFile

### B4 — `AsyncFile.open(path: &Path) -> some Future[Output = Result[AsyncFile, IoError]]`

```rx
use std.fs.AsyncFile

async def read_config() -> Result[String, IoError]
  let f = AsyncFile.open("/etc/hosts").await?
  f.read_to_string.await
end
```

Opens the file with `O_NONBLOCK`. The future immediately polls
ready (open(2) is non-blocking even without O_NONBLOCK on regular
files — but registering with the reactor for subsequent read/write
needs the flag).

### B5 — `AsyncFile.read_to_string -> some Future[Output = Result[String, IoError]]`

Reads until EOF. Yields `Pending` when `read()` returns `EAGAIN`;
the reactor's wake-on-readable fires when more data is available.

For v1 the read loop accumulates into a heap buffer and finally
returns the String. v2 could stream chunks.

### B6 — `AsyncFile.write_all(content: &str) -> some Future[Output = Result[nil, IoError]]`

Writes until either EOF on the writer or all of `content` is
flushed. Yields `Pending` when `write()` returns `EAGAIN`.

---

## Milestone 4C — AsyncTcpStream + AsyncTcpListener

Server work needs both sides. `AsyncTcpListener` ships in the same
milestone as `AsyncTcpStream`; they share the reactor registration
pattern (fd + EPOLLIN/EVFILT_READ) and read/write Drop discipline.

### B6.5 — `AsyncTcpListener.bind(addr: &str) -> some Future[Output = Result[AsyncTcpListener, IoError]]`

Wraps `socket(2)` + `bind(2)` + `listen(2)` with `O_NONBLOCK`. The
future resolves Ready immediately on success (bind doesn't block);
the async return is for surface symmetry with `AsyncTcpStream.connect`.

### B6.6 — `AsyncTcpListener.accept(&var self) -> some Future[Output = Result[(AsyncTcpStream, String), IoError]]`

Returns `(stream, peer_addr)` on success. Yields Pending when
`accept(2)` returns EAGAIN; reactor wakes on EPOLLIN. The accepted
stream is already non-blocking (inherits via `accept4(2)` on Linux,
explicit `fcntl(F_SETFL, O_NONBLOCK)` on macOS).

### B7 — `AsyncTcpStream.connect(addr: &str) -> some Future[Output = Result[AsyncTcpStream, IoError]]`

Non-blocking `connect(2)`. The future:
- First poll: `socket(2)` + `fcntl(F_SETFL, O_NONBLOCK)` + `connect(2)`.
  If immediate success, `Ready(Ok(stream))`. If `EINPROGRESS`,
  register with reactor for `EPOLLOUT` (Linux) / `EVFILT_WRITE`
  (macOS), return Pending.
- Subsequent poll: checks `getsockopt(SO_ERROR)` for connect
  completion. If 0, `Ready(Ok(stream))`. If non-zero, `Ready(Err)`.

### B8 — `AsyncTcpStream.read(&var self, buf: &var Array[Int]) -> some Future[Output = Result[Int, IoError]]`

Reads up to buf.size bytes. Returns the count read, or `Err(IoError)`
on failure. Yields Pending when read returns EAGAIN; reactor wakes
on EPOLLIN.

### B9 — `AsyncTcpStream.write(&var self, content: &str) -> some Future[Output = Result[Int, IoError]]`

Writes up to content.size bytes. Returns count written.

### B10 — `AsyncTcpStream.close(self) -> some Future[Output = nil]`

Shuts down the fd cleanly (`shutdown(SHUT_RDWR)` + `close`). The
`self` is consumed by-move.

### B11 — TCP echo e2e (full server + client round-trip)

E2E fixture: bind an `AsyncTcpListener`, accept one connection,
echo what's read, then a client connects, writes "hello", reads it
back, asserts equality. Round-trip in under 1s.

Practical setup: the listener side uses `Thread.spawn` (sync) to run
the accept loop in a separate thread for the test's duration —
single-threaded async can't do listener + client in the same
`block_on` without `task.spawn`, which doesn't ship until
sub-phase 5. Once sub-phase 5 lands, an e2e refactor moves the
listener + client into one `block_on` with two `task.spawn` calls;
4C ships the thread-bridged version.

### B12 — TCP echo drop / cleanup

Pin test: dropping a half-connected `AsyncTcpStream` cleanly
deregisters from the reactor + closes the fd. No leak under 1000
iterations.

---

## Cross-milestone

### B-X — Future drops deregister from reactor

When a future implementing reactor-aware I/O is dropped before
completion, its Drop hook calls `ruxen_reactor_deregister(handle)`
to clean up the registration. Same shape across timer / file /
stream futures.

### B-Y — Single-platform support: macOS (kqueue) + Linux (epoll)

The C runtime carries `#if defined(__APPLE__)` / `#if defined(__linux__)`
arms in `library/std/executor/runtime/reactor.c`. No Windows
(IOCP) for v1 — IOCP differs enough that it needs its own design
(v2 prompt).

### B-Z — `time.sleep(Duration)` works regardless of `block_on` context

The fn itself can be called in sync code via `block_on(sleep(d))`,
or in async code via `sleep(d).await`. Both paths drive the same
future.

---

## Pin tests

| Behaviour | Test fn                                              | File                          |
|-----------|------------------------------------------------------|-------------------------------|
| B1        | `time_sleep_returns_future_yielding_after_duration`  | `tests/async_io.rs`           |
| B2        | (covered by B1 e2e — reactor exercised end-to-end)   | —                             |
| B3        | `reactor_is_per_thread_singleton`                    | `tests/async_io.rs`           |
| B4        | `async_file_open_returns_future`                     | `tests/async_io.rs`           |
| B5        | `async_file_read_to_string_yields_until_eof`         | `tests/async_io.rs`           |
| B6        | `async_file_write_all_completes`                     | `tests/async_io.rs`           |
| B7        | `async_tcp_stream_connect_resolves`                  | `tests/async_io.rs`           |
| B8, B9    | `async_tcp_stream_read_write_round_trip`             | `tests/async_io.rs`           |
| B10       | `async_tcp_stream_close_completes`                   | `tests/async_io.rs`           |
| B1 e2e    | `cases/725_time_sleep_block_on.rx`                  | release-e2e                   |
| B4-B6 e2e | `cases/726_async_file_round_trip.rx`                | release-e2e                   |
| B11       | `cases/727_async_tcp_echo.rx`                       | release-e2e                   |
| B-X       | `future_drop_deregisters_from_reactor`               | `tests/async_io.rs`           |
| B12       | `tcp_stream_drop_no_leak_1000_iterations`            | `tests/async_io.rs`           |

---

## Out of scope (sub-phase 5)

- **`task.spawn` + `task.yield_now`** — multi-task within the
  single-threaded executor.
- **Multiple concurrent I/O in one block_on** — needs a real wake
  mechanism that knows which task to wake. v1 sub-phase 4 only has
  one task per block_on, so wake is "wake the executor's main poll
  loop".

## Out of scope (v2)

- **Windows IOCP support.**
- **Streaming reads / Bytes-like API.**
- **AsyncStdin / AsyncStdout / AsyncBufReader.** Same machinery
  applies; ship in a follow-up.
- **Async DNS resolution.** Sub-phase 4 uses synchronous `getaddrinfo`
  inside `AsyncTcpStream.connect`.

## Reserved error codes (new)

None new — async I/O failures all map to existing `IoError`
variants. The reactor's own setup-failure path uses E1116 if a
distinct code is desired (TBD on first impl).
