# Spec — `std.net`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.4](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_5_phase2_stdlib_io_completeness.md §4](../../prompts/v1/06_5_phase2_stdlib_io_completeness.md).

**Status:** shipped Phase 3 (minimal TCP surface) — Phase 2 #06.5 T5
adds typed `TcpListener` / `TcpStream` class wrappers + a `Shutdown`
enum on top.

`std.net` provides a minimal blocking TCP socket surface.  Addresses
are passed as `"host:port"` strings; the typed class wrappers own a
POSIX fd and `close` it on drop.

---

## Free-function surface (Phase 3)

### B1 — `tcp_connect(addr) -> Int`

Opens a TCP connection to `addr` and returns the connected file
descriptor on success.

**Given** `addr = "127.0.0.1:<unused-port>"` (no listener)
**When** the program calls `tcp_connect(&addr)`
**Then** the returned `Int` is **negative** (`< 0`) — by convention
`-1` for ECONNREFUSED / permission errors.

**Given** a peer is listening at `addr`
**Then** the returned `Int` is a **non-negative** file descriptor.

## B2 — `tcp_write(fd, bytes) -> Int`

Writes `bytes` to the connected socket; returns the number of bytes
written, or a negative value on error.  Partial writes are possible —
the caller must loop if it needs to write everything.

## B3 — `tcp_close(fd)` releases the socket

Closes the file descriptor.  Subsequent `tcp_read` / `tcp_write` on
the same fd return errors.  Idempotent at the runtime layer (closing
twice is safe).

## B4 — Round-trip: connect → write → close lands bytes at peer

**Given** a peer accepts on `127.0.0.1:<port>` and the Riven program
calls `connect → write("hello world") → close`
**Then** the peer reads exactly `"hello world"` from the stream.

## B5 — `tcp_listen` / `tcp_accept` / `tcp_read` surface

These are present in the runtime + resolver but only partially
pin-tested (the roundtrip test exercises connect+write+close from
the Riven side and connect+accept+read from the host side).

---

## Typed class surface (Phase 2 #06.5 T5)

`TcpListener` and `TcpStream` are flat 8-byte heap structs
`{ int32 fd; int32 closed }` mirroring `RivenFile`. Both participate
in the user_drop_classes pipeline so the MIR emits `<Type>_drop +
riven_dealloc` at scope exit, idempotent with explicit `.close()`.

```
class TcpListener
  def self.bind(addr: &String) -> Result[TcpListener, IoError]
  def accept(self) -> Result[TcpStream, IoError]        # blocking
  def local_addr(self) -> Result[String, IoError]
  def set_nonblocking(self, v: Bool) -> Result[(), IoError]
  def close(self) -> Result[(), IoError]                # also on drop
end

class TcpStream
  def self.connect(addr: &String) -> Result[TcpStream, IoError]
  def read(self, buf: &mut Array[U8]) -> Result[Int, IoError]
  def write(self, bytes: &Array[U8]) -> Result[Int, IoError]
  def peer_addr(self) -> Result[String, IoError]
  def shutdown(self, how: Shutdown) -> Result[(), IoError]
  def close(self) -> Result[(), IoError]                # also on drop
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

**Given** `addr = "0.0.0.0:1"` (privileged port, EACCES on most boxes)
**Then** the result is `Err(IoError.PermissionDenied | AddrInUse | …)`
with the corresponding errno-mapped kind.

### C2 — `TcpListener.close()` releases the fd

After `.close()` returns `Ok(())`, subsequent `.accept()` /
`.set_nonblocking(...)` / `.local_addr()` fail with
`Err(IoError.InvalidInput)`. Idempotent — repeated `.close()` is
`Ok(())`.

### C3 — `TcpListener` `drop` closes the fd

Letting a `TcpListener` go out of scope without an explicit `close`
still releases the fd — the drop pipeline emits
`TcpListener_drop(l) + riven_dealloc(l)`. A loop that binds + drops
N listeners must not exhaust the fd table.

### C4 — `TcpListener.local_addr() -> Result[String, IoError]`

Returns `Ok("127.0.0.1:<port>")` for an IPv4 listener. The port is
the kernel-chosen one when `bind` used `:0`. Closed listeners return
`Err(IoError.InvalidInput)`.

### C5 — `TcpListener.set_nonblocking(v: Bool) -> Result[(), IoError]`

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

### C8 — `TcpStream.write(bytes: &Array[U8]) -> Result[Int, IoError]`

Single `send(2)` call. Returns `Ok(n)` where `n` may be less than
`bytes.len` on partial writes; caller may loop or use a future
`write_all`. Returns `Err(IoError.BrokenPipe)` if the peer closed
mid-write.

### C9 — `TcpStream.read(buf: &mut Array[U8]) -> Result[Int, IoError]`

Single `recv(2)` call. Returns `Ok(0)` on clean EOF, `Ok(n>0)`
otherwise. Bytes are appended to `buf` (one int64 slot per byte,
matching `File.read`'s Vec[U8] convention).

### C10 — `TcpStream.peer_addr() -> Result[String, IoError]`

Returns `Ok("127.0.0.1:<port>")` for the remote end of a connected
IPv4 socket. After `.close()` returns `Err(IoError.InvalidInput)`.

### C11 — `TcpStream.shutdown(how: Shutdown) -> Result[(), IoError]`

Half-closes the stream:
- `Shutdown.Read`  — `SHUT_RD` — further `read` returns `Ok(0)` (EOF).
- `Shutdown.Write` — `SHUT_WR` — peer sees EOF on its `read`; local
  writes return `Err(IoError.BrokenPipe)`.
- `Shutdown.Both`  — `SHUT_RDWR` — both directions.

**E0713** — `Shutdown` variant unknown (tag outside 0..=2) is
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
against the runtime `RIVEN_SHUTDOWN_*` defines in
`library/runtime/net/tcp.c`.

### C15 — Auto-import from prelude

`TcpListener`, `TcpStream`, and `Shutdown` are importable via
`use std.net.{TcpListener, TcpStream, Shutdown}`. The class
declarations live in `library/std/src/net.rvn` (declarative doc —
executable behavior is wired in resolve/typeck/codegen).

---

## Pin tests

| Behaviour | Test fn                                          | File                           |
|-----------|--------------------------------------------------|--------------------------------|
| B1 (err)  | `tcp_connect_unreachable_returns_negative_one`   | `stdlib_net.rs`                |
| B1 (ok), B2, B3, B4 | `tcp_loopback_roundtrip`               | `stdlib_net.rs`                |
| B5        | covered indirectly by `blocking_tcp_echo_server_with_graceful_sigint_shutdown` | `stdlib_net.rs` |
| C1 (ok)   | `tcp_listener_class_bind_ok`                     | `stdlib_net.rs`                |
| C1 (err)  | `tcp_listener_class_bind_privileged_returns_err` | `stdlib_net.rs`                |
| C2        | `tcp_listener_class_close_idempotent`            | `stdlib_net.rs`                |
| C3        | `tcp_listener_class_drop_closes_fd`              | `stdlib_net.rs`                |
| C4        | `tcp_listener_class_local_addr`                  | `stdlib_net.rs`                |
| C5        | `tcp_listener_class_set_nonblocking_would_block` | `stdlib_net.rs`                |
| C6, C7 (ok), C8, C9 | `tcp_class_roundtrip`                  | `stdlib_net.rs`                |
| C7 (err)  | `tcp_stream_class_connect_unreachable_returns_err` | `stdlib_net.rs`              |
| C10       | `tcp_stream_class_peer_addr`                     | `stdlib_net.rs`                |
| C11       | `tcp_stream_class_shutdown_write_then_read_eof`  | `stdlib_net.rs`                |
| C12       | `tcp_stream_class_close_idempotent`              | `stdlib_net.rs`                |
| C13       | `tcp_stream_class_drop_closes_fd`                | `stdlib_net.rs`                |
| C14       | `shutdown_tag_values_match_runtime_and_resolver` | `shutdown_tag_stability.rs`    |
| C15       | `tcp_class_prelude_auto_import_resolves`         | `stdlib_net.rs`                |

---

## Out of scope (v2)

- UDP, Unix domain sockets, TLS.
- IPv6 literal addresses (the runtime uses `getaddrinfo`, so they may
  work, but no pin test asserts it).
- `accept` returning the peer `SocketAddr` alongside the stream — the
  v1 method returns only the stream; `peer_addr` recovers the address
  on demand.
- `TcpListener.incoming() -> Iterator[Result[TcpStream, IoError]]` —
  ergonomic loop helper; user code today writes the explicit `loop
  match listener.accept() ... end` form.
