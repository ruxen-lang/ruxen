# Spec — `std.net`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.4](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_5_phase2_stdlib_io_completeness.md §4](../../prompts/v1/06_5_phase2_stdlib_io_completeness.md).

**Status:** shipped Phase 2 #06.5 T5 — class-only TCP surface.

`std.net` provides a minimal blocking TCP socket surface. Addresses
are passed as `"host:port"` strings; the class wrappers own a POSIX
fd and `close` it on drop. The flat fd-based `tcp_*` free functions
from the prompt-#06.5-T5 predecessor (Phase 3) have been **removed**
from the user-facing surface, and their C runtime symbols
(`ruxen_tcp_connect`, `ruxen_tcp_listen`, …) were deleted from
`tcp.c`.

---

## Typed class surface

`TcpListener` and `TcpStream` are flat 8-byte heap structs
`{ int32 fd; int32 closed }` mirroring `RuxenFile`. Both participate
in the user_drop_classes pipeline so the MIR emits `<Type>_drop +
ruxen_dealloc` at scope exit, idempotent with explicit `.close()`.

```
class TcpListener
  def self.bind(addr: &String) -> Result[TcpListener, IoError]
  def accept -> Result[TcpStream, IoError]              # blocking
  def local_addr -> Result[String, IoError]
  def set_nonblocking(v: Bool) -> Result[nil, IoError]
  def close -> Result[nil, IoError]                      # also on drop
end

class TcpStream
  def self.connect(addr: &String) -> Result[TcpStream, IoError]
  def read(buf: &var Array[Int]) -> Result[Int, IoError]
  def write(bytes: &Array[Int]) -> Result[Int, IoError]
  def peer_addr -> Result[String, IoError]
  def shutdown(how: Shutdown) -> Result[nil, IoError]
  def set_read_timeout(d: &Duration) -> Result[nil, IoError]
  def set_write_timeout(d: &Duration) -> Result[nil, IoError]
  def close -> Result[nil, IoError]                      # also on drop
end

enum Shutdown
  Read
  Write
  Both
end
```

### C1 — `TcpListener.bind(addr) -> Result[TcpListener, IoError]`

**Given** `addr = "127.0.0.1:0"` (kernel-assigned ephemeral port)
**When** the program calls `TcpListener.bind(&addr)`
**Then** the result is `Ok(listener)` and the listener owns the
underlying fd.

**Given** a malformed `addr` (no colon, junk port, …)
**Then** the result is `Err(IoError.InvalidInput)`.

### C2 — `TcpListener.close()` releases the fd

After `.close()` returns `Ok(nil)`, subsequent `.accept()` /
`.set_nonblocking(...)` / `.local_addr()` fail with
`Err(IoError.InvalidInput)`. Idempotent — repeated `.close()` is
`Ok(nil)`.

### C3 — `TcpListener` `drop` closes the fd

Letting a `TcpListener` go out of scope without an explicit `close`
still releases the fd — the drop pipeline emits
`TcpListener_drop(l) + ruxen_dealloc(l)`. A loop that binds + drops
N listeners must not exhaust the fd table.

### C4 — `TcpListener.local_addr() -> Result[String, IoError]`

Returns `Ok("127.0.0.1:<port>")` for an IPv4 listener. The port is
the kernel-chosen one when `bind` used `:0`. Closed listeners return
`Err(IoError.InvalidInput)`.

### C5 — `TcpListener.set_nonblocking(v: Bool) -> Result[nil, IoError]`

Flips `O_NONBLOCK` on the underlying fd. After
`set_nonblocking(true)`, `accept()` on an idle listener returns
`Err(IoError.WouldBlock)` instead of blocking.

### C6 — `TcpListener.accept() -> Result[TcpStream, IoError]`

Blocks until a peer connects. On success returns `Ok(stream)`
wrapping the new client fd. On `EINTR` (e.g. SIGINT delivered during
the call) returns `Err(IoError.Interrupted)` so cooperative shutdown
loops can break out.

### C7 — `TcpStream.connect(addr) -> Result[TcpStream, IoError]`

**Given** `addr = "127.0.0.1:<unused-port>"` (no listener)
**Then** the result is `Err(IoError.ConnectionRefused)`.

**Given** a peer is listening at `addr`
**Then** the result is `Ok(stream)` with the connected client fd.

### C8 — `TcpStream.write(bytes: &Array[Int]) -> Result[Int, IoError]`

Single `send(2)` call. Returns `Ok(n)` where `n` may be less than
`bytes.size` on partial writes; caller may loop or use a future
`write_all`. Returns `Err(IoError.BrokenPipe)` if the peer closed
mid-write.

### C9 — `TcpStream.read(buf: &var Array[Int]) -> Result[Int, IoError]`

Single `recv(2)` call. Returns `Ok(0)` on clean EOF, `Ok(n>0)`
otherwise. Bytes are appended to `buf` (one int64 slot per byte,
matching `File.read`'s byte-array convention).

**Binary-safety:** the read path is genuinely binary-safe — embedded
`0x00` bytes round-trip via `Array[Int]`. The implementation routes
through the new `ruxen_tcp_read_bytes(fd, buf, max)` runtime helper
which writes each received byte into a fresh int64 slot, never
treating the staging buffer as a C string. The legacy
`ruxen_tcp_read(fd, max) -> char*` helper (which DID truncate on
embedded NULs) was deleted with the rest of the Phase-3 free fns.

### C10 — `TcpStream.peer_addr() -> Result[String, IoError]`

Returns `Ok("127.0.0.1:<port>")` for the remote end of a connected
IPv4 socket. After `.close()` returns `Err(IoError.InvalidInput)`.

### C11 — `TcpStream.shutdown(how: Shutdown) -> Result[nil, IoError]`

Half-closes the stream:
- `Shutdown.Read`  — `SHUT_RD` — further `read` returns `Ok(0)` (EOF).
- `Shutdown.Write` — `SHUT_WR` — peer sees EOF on its `read`; local
  writes return `Err(IoError.BrokenPipe)`.
- `Shutdown.Both`  — `SHUT_RDWR` — both directions.

**E0713** — `Shutdown` variant unknown (tag outside 0..2) is
surfaced as `Err(IoError.InvalidInput)` with message `"E0713 Shutdown
variant unknown"`. The runtime only emits this if the enum tagged-
value layout has drifted; in normal compiled code the typeck arm
guarantees the tag is one of `{0, 1, 2}`.

### C12 — `TcpStream.close()` releases the fd

Same idempotency contract as `TcpListener.close` (C2). Subsequent
ops on a closed stream return `Err(IoError.InvalidInput)`.

### C13 — `TcpStream` `drop` closes the fd

Same drop-pipeline story as C3. A loop that connects + drops N
streams must not exhaust fds.

### C14 — `Shutdown` enum tag stability

Tag values are pinned: `Read=0`, `Write=1`, `Both=2` — pin-tested
against the runtime `RUXEN_SHUTDOWN_*` defines in
`library/std/net/runtime/tcp.c`.

### C15 — Auto-import from prelude

`TcpListener`, `TcpStream`, and `Shutdown` are importable via
`use std.net.{TcpListener, TcpStream, Shutdown}`. The class
declarations live in `library/std/net/src/lib.rx` (declarative doc —
executable behavior is wired in resolve/typeck/codegen).

### C17 — `TcpStream.set_read_timeout(d: &Duration) -> Result[nil, IoError]`

Sets the `SO_RCVTIMEO` socket option to the given Duration. Once the
timeout fires a subsequent blocking `.read(...)` returns
`Err(IoError.WouldBlock)` instead of blocking forever. Passing a
Duration with `nanos == 0` clears the timeout. Sub-microsecond non-zero
Durations round up to 1µs so a "set as short as possible" intent
isn't accidentally interpreted as "clear".

### C18 — `TcpStream.set_write_timeout(d: &Duration) -> Result[nil, IoError]`

Sets the `SO_SNDTIMEO` socket option. Same semantics as
`set_read_timeout` but applied to writes — a `.write(...)` that
can't drain to the kernel buffer within the budget returns
`Err(IoError.WouldBlock)`.

### C19 — Binary-safe read round-trip

A byte sequence containing `0x00` (e.g. `[0xFF, 0x00, 0x41, 0x00, 0x42]`)
sent via `TcpStream.write(&bytes)` and read back via `TcpStream.read(
&var buf)` round-trips with all bytes intact — no truncation at the
embedded NUL. Pinned by "reads embedded zero bytes" in
`library/std/net/tests/tcp_test.rx`.

### C16 — Flat `tcp_*` free fns are no longer exposed

A Ruxen program that writes `use std.net.tcp_connect` must fail at
resolve time with "name not found in std.net". The 6 C runtime
symbols (`ruxen_tcp_connect`, `ruxen_tcp_listen`,
`ruxen_tcp_accept`, `ruxen_tcp_read`, `ruxen_tcp_write`,
`ruxen_tcp_close`) were deleted from the runtime.

---

## Pin tests

| Behaviour | Test fn                                            | File                           |
|-----------|----------------------------------------------------|--------------------------------|
| C1 (ok)   | "binds to port 0 and reports a 127.0.0.1 local_addr" | `library/std/net/tests/tcp_test.rx` |
| C1 (err)  | "returns Err from bind for a malformed address" | `library/std/net/tests/tcp_test.rx` |
| C2        | "closes idempotently, then local_addr is Err" | `library/std/net/tests/tcp_test.rx` |
| C3        | "releases the fd on drop across 200 binds" | `library/std/net/tests/tcp_test.rx` |
| C4        | "binds to port 0 and reports a 127.0.0.1 local_addr" | `library/std/net/tests/tcp_test.rx` |
| C5        | "returns WouldBlock from accept when nonblocking and idle" | `library/std/net/tests/tcp_test.rx` |
| C6, C7 (ok), C8, C9 | "delivers written bytes to the accepted peer, then EOF after close" | `library/std/net/tests/tcp_test.rx` |
| C7 (err)  | "returns Err from connect when nothing listens" | `library/std/net/tests/tcp_test.rx` |
| C10       | "reports the listener as peer_addr" | `library/std/net/tests/tcp_test.rx` |
| C11       | "shows EOF to the peer after shutdown(Write)" | `library/std/net/tests/tcp_test.rx` |
| C12       | "closes idempotently" | `library/std/net/tests/tcp_test.rx` |
| C13       | "releases the fd on drop across 200 connections" | `library/std/net/tests/tcp_test.rx` |
| C14       | `shutdown_tag_values_match_runtime_and_resolver`   | `shutdown_tag_stability.rs`    |
| C15       | "the `use std.net.{TcpListener, TcpStream, Shutdown}` line" | `library/std/net/tests/tcp_test.rx` |
| C16       | retired syntax, no pin                             |                                |
| C17       | "returns WouldBlock from read after the read timeout on an idle peer" | `library/std/net/tests/tcp_test.rx` |
| C18       | "accepts a write timeout" | `library/std/net/tests/tcp_test.rx` |
| C19       | "reads embedded zero bytes" | `library/std/net/tests/tcp_test.rx` |

The SIGINT echo server test stays in `stdlib_net.rs`
(`blocking_tcp_echo_server_with_graceful_sigint_shutdown`): the host
process has to send the signal.

---

## Out of scope (v2)

- UDP, Unix domain sockets, TLS.
- IPv6 literal addresses (the runtime uses `getaddrinfo`, so they may
  work, but no pin test asserts it).
- `accept` returning the peer `SocketAddr` alongside the stream — the
  v1 method returns only the stream; `peer_addr` recovers the address
  on demand.
- A block helper that yields each accepted connection — user code today
  writes the explicit `loop match listener.accept ... end` form.
