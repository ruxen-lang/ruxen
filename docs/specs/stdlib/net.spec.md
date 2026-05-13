# Spec — `std::net`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.4](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 3 (minimal TCP surface).

`std::net` provides a minimal blocking TCP socket surface.  Addresses
are passed as `"host:port"` strings; sockets are file descriptors
(`Int`); failure modes are signalled via negative return values.

---

## B1 — `tcp_connect(addr) -> Int`

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

## Pin tests

| Behaviour | Test fn                                       | File             |
|-----------|-----------------------------------------------|------------------|
| B1 (err)  | `tcp_connect_unreachable_returns_negative_one`| `stdlib_net.rs`  |
| B1 (ok), B2, B3, B4 | `tcp_loopback_roundtrip`            | `stdlib_net.rs`  |
| B5        | gap — see below                               |                  |

---

## Gaps (add pin tests when next touched)

- B5: Riven-side `tcp_listen` → `tcp_accept` → `tcp_read` round-trip
  (today the test uses the host's listener, so the Riven side is only
  exercised as a client).

## Out of scope (v2)

- Typed wrappers (`TcpStream`, `TcpListener` classes that own the fd
  and `drop` it on scope exit).  v1 exposes raw fds.
- UDP, Unix domain sockets, TLS.
- Non-blocking / async I/O — v1 is blocking sockets only.
- IPv6 literal addresses (the runtime uses `getaddrinfo`, so they may
  work, but no pin test asserts it).
