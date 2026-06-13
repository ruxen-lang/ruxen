# 07-wasm-heap — heap types (`Array`/`String`) on wasm

Tier 4.09. Where `05-wasm` proved *pure-math* wasm (no heap), this proves the
**heap** works on `wasm32-unknown-unknown`: `array_sum` builds a heap-allocated
`Array` (`ruxen_vec_new` + `push` over a bundled wasm allocator) and sums it.

This is the compiler/runtime foundation for running the canvas+quiver GUI stack
in the browser — a GUI app is all `String`/`Array`, none of which worked on wasm
before this.

## Build + run

```bash
# Toolchain must be built with --features llvm, and clang/wasm-ld (LLVM 18) on PATH.
LLVM_SYS_180_PREFIX=/usr/lib64/llvm18 PATH="/usr/lib64/llvm18/bin:$PATH" \
  cargo build -p ruxen_cli --features llvm

target/debug/ruxen compile examples/07-wasm-heap/heap.rx \
  --target wasm32-unknown-unknown -o examples/07-wasm-heap/heap.wasm

node examples/07-wasm-heap/run.mjs examples/07-wasm-heap/heap.wasm
# => ok  array_sum() = 42 (want 42) / heap on wasm works
```

`scripts/wasm_verify.sh` runs this end-to-end (alongside the `05-wasm` math bar).

## How it works

The wasm path compiles a curated heap-core stdlib subset (`core`, `array`,
`string`, …) — excluding the `dispatch runtime` / libc-heavy modules the LLVM
backend or a no-libc target can't handle — and links the heap-core C runtime
(`alloc.c`, `vec.c`) plus a bundled allocator/libc shim. The result instantiates
with **no imports**: the module carries its own allocator. `array_sum` returns an
`i64`, which JS surfaces as a `BigInt`. See
`docs/requirements/tier4_09_wasm_heap_and_host_imports.md`.
