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
`codegen::compile_*` entry points called by `src/ruxenc`, plus
`compile_with_runtime_sources` (the host link with a test-supplied runtime in
place of the stdlib one, used by `drop_fixtures`). The two runtime ABI
tables (`cranelift/runtime_sigs.rs`, `llvm/runtime_decl.rs`) are an ABI **contract**
with the C runtime — every signature must match the C functions in `library/`.

## Depends on
MIR (`mir::MirProgram`), `target` policy, external tools (`cc`/`clang`/`wasm-ld`/
the platform linker), and the C runtime in `library/`. No upward edges.

## Invariants & gotchas
- Strict downward order: codegen is the bottom of `lexer→…→codegen`; never reach up.
- ABI: everything crosses as `int64_t` except `Float` (C `double`); `&String` is
  `char*`. A `&var T` is a slot address only for String, else the value's own
  word: ask `mir::refmut_abi::refmut_is_slot_address` (Cranelift, LLVM and the
  REPL JIT `RefMut` pre-scans all do). Changing a runtime signature means editing BOTH ABI tables + the C side.
- **The erased uniform word (B01) — read before touching any float coercion.**
  A value whose static type codegen cannot see lives in a 64-bit *uniform
  word*: a generic-collection slot (`RuxenVec` / the hash+set tables are
  `int64_t` regardless of `T`), an address-taken local's 8-byte stack slot, a
  param/return whose declared `Ty` is opaque, a closure's params/return when
  reached through an erased `Fn` box, and a scalar method home's `self` (typed
  as the FFI shell `Class{"Float"}`). **A `Float` in a uniform word carries its
  IEEE-754 bit pattern, never an `fcvt` numeric conversion** — `coerce_erased_word`,
  not `coerce_value`. `Float32` is its own 32-bit pattern zero-extended into the
  word. Both directions must move together; half the fix is worse than none.
  - The distinction `coerce_value` CANNOT make: a declared `Int` param and an
    erased `T` param are both `i64`. Implicit numeric conversion into a
    *declared* scalar param (`f(3)` where `def f(x: Float)`) must stay `fcvt`.
    So the decision is keyed on the DECLARED Ruxen type via
    `codegen::is_erased_word_ty`, never on the machine type. Pin
    `tests/release-e2e/cases/1000_float_erased_word.rx` guards both that line
    (its `run_1004` section) and the erasure side.
  - The CLOSURE half lives in MIR lowering (`mir/lower/expr/closure.rs` and
    `expr/method_call.rs`, via `FloatToBits` / `BitsToFloat`), deliberately, so
    both backends inherit one implementation instead of each re-deriving it.
  - The LLVM mirror of `coerce_erased_word` was written but NOT compile-verified
    (LLVM 18 absent where B01 landed; `llvm-sys` fails to build). Expect
    mechanical fixes there on the first `--features llvm` build.
- Cranelift is hard-blocked for wasm/embedded (`requires_llvm_backend()`); those
  targets force LLVM (`mod.rs` guard).
- The wasm path (`compile_cross` → `target.is_wasm()`) now compiles a curated
  runtime subset (`object::WASM_RUNTIME_CORE`: alloc.c, vec.c) + a bundled
  allocator/libc shim (`object::WASM_RT_C`) via clang `--target=wasm32` and links
  them (`emit_wasm_module` takes runtime objects). `runtime_decl.rs` declares the
  vec ABI with int64 slots. Tier 4.09 (WIP) — see
  `docs/requirements/tier4_09_wasm_heap_and_host_imports.md`. KNOWN-OPEN: on wasm32
  the call-site arg lowering passes an `Int` to `ruxen_vec_push` as i32 instead of
  i64 (slot-width bug) → traps; fix is in `llvm/emit` arg lowering, not the decl table.
