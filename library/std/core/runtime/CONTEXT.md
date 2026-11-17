# core/runtime — the C runtime floor (alloc + cross-TU ABI types)

The lowest layer of the C runtime: the allocator and the shared types/asserts every
other stdlib `runtime/*.c` builds on. Compiled into `libruxenrt.a` (native) and
linked by codegen.

## What lives here
- `runtime.h` — the cross-TU header: platform asserts, `RuxenVec` (the owning
  growable `int64_t`-slot array used everywhere), `RuxenFdHandle` (the
  `{fd, closed}` layout of File/TcpStream/TcpListener/AsyncFile), the core
  function decls, and the shared `static inline` helpers every package uses
  instead of a private copy: Result/Option builders (`ruxen_result_*_value`,
  `ruxen_option_some`/`_none`, `ruxen_io_error_invalid_input`,
  `ruxen_io_error_write_zero`), `ruxen_xmalloc`,
  `RuxenByteBuf`, `ruxen_utf8_encode`/`_decode`, and `ruxen_fd_close_retry` /
  `ruxen_fd_handle_close` (host only).
- `alloc.c` — `ruxen_alloc` (zeroing `malloc` wrapper, panic-on-OOM) and
  `ruxen_string_ORIG_FREE` / free helpers. The single allocation seam.
- `repl_replay.c` — REPL JIT replay support. (Test-only externs live in `library/test_std/`, outside the production runtime.)

## Public surface
`ruxen_alloc`, the `Ruxen*` struct layouts (esp. `RuxenVec { int64_t* data; len; cap }`),
and the panic/free helpers. These names appear in the codegen ABI tables
(`codegen/cranelift/runtime_sigs.rs`, `codegen/llvm/runtime_decl.rs`) — this header
is one side of that contract.

## Depends on
libc (`malloc`/`free`/`memset`) on native targets. Nothing in the Ruxen tree.

## Invariants & gotchas
- One copy per helper: a package needing an Option/Result box, an OOM-checked
  `malloc`, a growable byte buffer, a UTF-8 codec, an fd close or an errno →
  IoError tag (`ruxen_io_error_classify_errno`, io/runtime/io_error.c) uses the
  shared one, never a private copy.
- A runtime that reports an IoError later than the failing call (the async
  state machines) stores the errno, not the tag, and builds the error with
  `ruxen_io_error_from_errno` / `ruxen_io_error_value_from_errno` so the
  strerror message survives.
- A collection slot's ownership travels as an element descriptor (`RUXEN_ELEM_*`
  in `runtime.h`, built only by `mir/lower/drops/` `elem_desc`).
  `ruxen_elem_clone` and `ruxen_elem_drop` (array/runtime/vec.c) must follow
  the same tags to the same depth, or a deep copy double-frees or leaks.
  `RUXEN_ELEM_OBJ` (a user class/struct slot) is always the last tag: the rest
  of the word is the address of compiler-synthesised element glue
  (`int64_t glue(op, slot)`: drop or clone), so the runtime never needs to
  know a user type. `RUXEN_ELEM_CLOSURE` (a closure slot) is a leaf: dropping
  calls the pair's own drop slot (word 2), copying adds an owner (word 3) and
  returns the same pair.
- Helpers keep the literal `malloc(` / `realloc(` / `free(` spellings: the
  drop_fixtures test rewrites them textually to count raw heap traffic.
- ABI: a "slot" is `int64_t`; pointers (`char*`, heap ptrs) are reinterpret-cast
  into it. `RuxenVec.data` is `int64_t*`.
- `_Static_assert(sizeof(void*) == sizeof(int64_t))` hard-requires a 64-bit target —
  **it fails to compile on wasm32** and is gated behind `!defined(__wasm32__)` for
  the wasm runtime build (tier 4.09). wasm is little-endian, so the 32-bit-ptr-in-
  64-bit-slot pattern still round-trips.
- `ruxen_alloc` calls libc `malloc`, absent on `wasm32-unknown-unknown`; the wasm
  build supplies a bundled allocator (dlmalloc) — see
  `docs/requirements/tier4_09_wasm_heap_and_host_imports.md`.
