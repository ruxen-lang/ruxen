# 08-wasm-import — calling host (JS) functions from wasm

Tier 4.09. The other half of the wasm foundation (with `07-wasm-heap`): a Ruxen
wasm module can **call into the host**. A top-level `lib "<module>"` block on the
wasm target declares functions the host supplies — the lib name is the wasm import
**module**, each `def name as "<symbol>"` the import **field**. So:

```ruxen
lib "env"
  def host_mul as "host_mul"(a: Int, b: Int) -> Int
end
def compute -> Int
  host_mul(6, 7)
end
```

compiles to a module that imports `env.host_mul` and exports `compute`. Use any
module name (`lib "canvas"`, `lib "console"`, …) to organize host APIs — that's
how the browser canvas backend will call browser/JS functions.

## Build + run

```bash
LLVM_SYS_180_PREFIX=/usr/lib64/llvm18 PATH="/usr/lib64/llvm18/bin:$PATH" \
  cargo build -p ruxen_cli --features llvm

target/debug/ruxen compile examples/08-wasm-import/import.rx \
  --target wasm32-unknown-unknown -o examples/08-wasm-import/import.wasm

node examples/08-wasm-import/run.mjs examples/08-wasm-import/import.wasm
# => ok  compute() = 42 (want 42) / host import called: 6*7 in JS
```

The host supplies the import:
`WebAssembly.instantiate(bytes, { env: { host_mul: (a, b) => a * b } })`.
`i64` params/returns cross as JS `BigInt`. `scripts/wasm_verify.sh` runs this as a
third e2e bar. See `docs/requirements/tier4_09_wasm_heap_and_host_imports.md`.

## How it works

On wasm, `declare_ffi_function` (LLVM backend) tags each top-level `lib` function
with `wasm-import-module`/`wasm-import-name` attributes, so `wasm-ld` emits a
precise `<module>.<field>` import. The stdlib's own `ruxen_*` runtime bindings are
class-body FFI (resolved by the linked runtime), not top-level libs, so they are
not turned into imports.
