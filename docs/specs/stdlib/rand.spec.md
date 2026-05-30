# Spec — `std.rand`

**Status:** shipped Phase 2 #06.5 T8 — kernel CSPRNG-backed surface.

CSPRNG-only. No userspace PRNG, no seeding — each call delegates to
the OS kernel. Pin tests in `compiler/ruxen_core/tests/stdlib_rand.rs`
are the canonical docs for behaviour.

- `random_bytes(n: Int) -> Result[Array[U8], IoError]` — returns
  `Ok(buf)` with `buf.len == n` filled from the kernel CSPRNG;
  `Ok(empty)` when `n == 0`; `Err(IoError.InvalidInput)` when `n < 0`;
  `Err(IoError.Other(msg))` on CSPRNG hard failure (never panics).
- `random_u64() -> Int` — returns 64 random bits in an int64 carrier
  (same convention as `now_ns` / `unix_ns`); panics on hard CSPRNG
  failure.
- `random_fill(buf: &var Array[U8]) -> Result[(), IoError]` —
  overwrites every existing slot of `buf` with one random byte each;
  preserves `buf.len`; `Ok(())` when `buf` is empty.

**Backends** (compile-time `#if` selected, not user-visible):
- Linux: `getrandom(buf, len, 0)` from `<sys/random.h>`, EINTR retry.
- macOS: `SecRandomCopyBytes(kSecRandomDefault, len, buf)` from
  `<Security/Security.h>`. Requires `-framework Security` at link.
- Fallback Unix: `/dev/urandom` open + read-loop.

**Type-system note:** the resolver / typeck use `Array[Int]` for the
byte carrier — same convention as `File.read_all` and `TcpStream.read`
(RuxenVec slots are int64). The user-visible `Array[U8]` in the prose
above is the declarative spec spelling.
