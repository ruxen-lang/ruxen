# 05 — Ruxen → WebAssembly

Compiles a Ruxen source to `wasm32-unknown-unknown` and calls its exports from
Node.js. Demonstrates tier 4.03 (the WASM target).

## What it shows

- `ruxen compile --target wasm32-unknown-unknown` routes to the LLVM backend
  and emits a `.wasm` module via `wasm-ld` (no libc, no C runtime — a *reactor*
  module).
- Every top-level free `def` becomes a host-callable wasm **export** under its
  source name (via the LLVM `export_name` attribute — no name mangling leaks
  into the export).
- A hand-written JS loader (`run.mjs`) instantiates the module and calls the
  exports directly over the C ABI (`i32` in / `i32` out).

## Run it

```bash
# 1. Build the toolchain with the LLVM backend (wasm needs LLVM 18):
LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18 \
  cargo build -p ruxen_cli --features llvm

# 2. Compile the Ruxen source to wasm:
target/debug/ruxen compile examples/05-wasm/add.rx \
  --target wasm32-unknown-unknown -o examples/05-wasm/add.wasm

# 3. Run it in Node:
node examples/05-wasm/run.mjs examples/05-wasm/add.wasm
```

Expected output:

```
ok   add(2, 3) = 5 (want 5)
ok   mul(6, 7) = 42 (want 42)
ok   square(9) = 81 (want 81)
ok   add(-4, 4) = 0 (want 0)
all exports correct
```

The scripted verifier `scripts/wasm_verify.sh` does all three steps and asserts
exit 0.

## Scope (v1)

This is the **pure-computation** path: integer math, no heap, no I/O. Exporting
`String`/`Array` (needs a bundled allocator) and importing host functions
(`console_log` via `wasm_import`) are the staged remainder — see
`docs/decisions/phase4-no-std-wasm.md` and
`docs/requirements/tier4_03_wasm_target.md`.
