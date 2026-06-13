# core/runtime — the C runtime floor (alloc + cross-TU ABI types)

The lowest layer of the C runtime: the allocator and the shared types/asserts every
other stdlib `runtime/*.c` builds on. Compiled into `libruxenrt.a` (native) and
linked by codegen.

## What lives here
- `runtime.h` — the cross-TU header: platform asserts, `RuxenVec` (the owning
  growable `int64_t`-slot array used everywhere), and the core function decls.
- `alloc.c` — `ruxen_alloc` (zeroing `malloc` wrapper, panic-on-OOM) and
  `ruxen_string_ORIG_FREE` / free helpers. The single allocation seam.
- `repl_replay.c` — REPL JIT replay support. `test_extern.c` — test-only externs.

## Public surface
`ruxen_alloc`, the `Ruxen*` struct layouts (esp. `RuxenVec { int64_t* data; len; cap }`),
and the panic/free helpers. These names appear in the codegen ABI tables
(`codegen/cranelift/runtime_sigs.rs`, `codegen/llvm/runtime_decl.rs`) — this header
is one side of that contract.

## Depends on
libc (`malloc`/`free`/`memset`) on native targets. Nothing in the Ruxen tree.

## Invariants & gotchas
- ABI: a "slot" is `int64_t`; pointers (`char*`, heap ptrs) are reinterpret-cast
  into it. `RuxenVec.data` is `int64_t*`.
- `_Static_assert(sizeof(void*) == sizeof(int64_t))` hard-requires a 64-bit target —
  **it fails to compile on wasm32** and is gated behind `!defined(__wasm32__)` for
  the wasm runtime build (tier 4.09). wasm is little-endian, so the 32-bit-ptr-in-
  64-bit-slot pattern still round-trips.
- `ruxen_alloc` calls libc `malloc`, absent on `wasm32-unknown-unknown`; the wasm
  build supplies a bundled allocator (dlmalloc) — see
  `docs/requirements/tier4_09_wasm_heap_and_host_imports.md`.
