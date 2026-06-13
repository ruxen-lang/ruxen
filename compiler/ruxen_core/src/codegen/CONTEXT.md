# codegen — MIR → machine code / object emission / linking

The final compiler phase: lower `MirProgram` to a native or wasm object, then link
to an executable / `.wasm`. Two backends behind one `Backend` enum.

## What lives here
- `mod.rs` — entry points + orchestration: host path (`libruxenrt.a` fast path or
  compile-runtime fallback) and `compile_cross` (cross/wasm targets). Picks backend,
  finds/links the runtime, resolves the linker. Houses the wasm branch.
- `target.rs` — `ResolvedTarget` + triple parsing/aliases, `LinkerSpec`,
  `requires_llvm_backend()` / `is_wasm()` / `is_darwin()`. The target-policy hub.
- `object.rs` — turning object bytes into files + invoking external tools: `cc`
  (`compile_runtime*`), the system/cross linkers, and `emit_wasm_module` (`wasm-ld`).
  External-process discovery (cc/clang/wasm-ld, `RUXEN_WASM_LD`) lives here.
- `cranelift/` — default backend (dev + REPL JIT); native ISAs only, **cannot emit
  wasm**. `runtime_sigs.rs` is its runtime ABI table.
- `llvm/` — feature `llvm` (Inkwell + LLVM 18); release + the **only wasm-capable**
  backend. `runtime_decl.rs` is its runtime ABI table; `emit/` sets export/import attrs.
- `runtime/mod.rs` — locating the embedded/host runtime C sources.
- `layout.rs` — type/struct layout. `lang_intrinsics.rs` — compiler-known intrinsics.

## Public surface
`codegen::compile_*` entry points called by `ruxen_driver`. The two runtime ABI
tables (`cranelift/runtime_sigs.rs`, `llvm/runtime_decl.rs`) are an ABI **contract**
with the C runtime — every signature must match the C functions in `library/`.

## Depends on
MIR (`mir::MirProgram`), `target` policy, external tools (`cc`/`clang`/`wasm-ld`/
the platform linker), and the C runtime in `library/`. No upward edges.

## Invariants & gotchas
- Strict downward order: codegen is the bottom of `lexer→…→codegen`; never reach up.
- ABI: everything crosses as `int64_t` except `Float` (C `double`); `&String` is
  `char*`. Changing a runtime signature means editing BOTH ABI tables + the C side.
- Cranelift is hard-blocked for wasm/embedded (`requires_llvm_backend()`); those
  targets force LLVM (`mod.rs` guard).
- The wasm path (`compile_cross` → `target.is_wasm()`) historically emitted a bare
  reactor module (no runtime). Tier 4.09 changes this to link a core runtime subset
  + bundled allocator — see `docs/requirements/tier4_09_wasm_heap_and_host_imports.md`.
