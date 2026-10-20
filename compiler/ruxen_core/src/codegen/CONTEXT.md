# codegen — MIR → machine code / object emission / linking

The final compiler phase: lower `MirProgram` to a native or wasm object, then link
to an executable / `.wasm`. Two backends behind one `Backend` enum.

## What lives here
- `backend.rs` — `Backend::select` (the one selection rule for every driver:
  override, `--release`, opt level, a target that needs LLVM, the `llvm`-feature
  error) and `Backend::emit_object` (the one dispatch to a backend's `CodeGen`).
- `mod.rs` — entry points + orchestration: `LinkPlan` + `link_host` (the one host
  link: `libruxenrt.a` fast path or compile-runtime fallback, used by
  `compile_with_options*` and ruxenc), `aggregate_link_flags`, and
  `compile_cross` (cross/wasm targets). Finds/links the runtime, resolves the
  linker. Houses the wasm branch.
- `target.rs` — `ResolvedTarget` + triple parsing/aliases, `LinkerSpec`,
  `requires_llvm_backend()` / `is_wasm()` / `is_darwin()`. The target-policy hub.
- `object.rs` — turning object bytes into files + invoking external tools: `cc`
  (`compile_runtime*`), the system/cross linkers, and `emit_wasm_module` (`wasm-ld`).
  External-process discovery (cc/clang/wasm-ld, `RUXEN_WASM_LD`) lives here.
- The lowering policy is MIR's (`mir/abi.rs`: `AbiClass`, `FnAbi`, `CalleeIndex`,
  the B01 predicates, every coercion decision; `mir/residual_abi.rs`: the runtime
  symbols the compiler emits itself; `mir/lang_intrinsics.rs`: `runtime_name`).
  `mir::coercion` applies the decisions once as `MirInst::Convert` and builds
  `MirProgram::callees`; a backend maps a class to a native type, emits each
  `Convert` as given and declares callees from that index.
- `lower.rs` — the one MIR walk (`lower_function<B: IrBuilder>`), every
  instruction decision; `ir_builder.rs` — the `IrBuilder` trait a backend
  implements (machine operations only; generic, so monomorphised per backend).
- `cranelift/` — default backend (dev + REPL JIT); native ISAs only, **cannot emit
  wasm**. `helpers.rs` maps `AbiClass` to Cranelift types (a pointer is `I64`).
- `llvm/` — feature `llvm` (Inkwell + LLVM 18); release + the **only wasm-capable**
  backend. `types.rs` maps `AbiClass` to LLVM types; `emit/` declares every
  `CalleeIndex` import up front, sets export attrs and lowers each function
  through the walk (`emit/builder.rs`, `LlvmBuilder`).
- `layout.rs` — type/struct layout.

## Public surface
`codegen::compile_*` entry points called by `src/ruxenc`, plus
`compile_with_runtime_sources` (the host link with a test-supplied runtime in
place of the stdlib one, used by `drop_fixtures`). `LinkInputs::for_project` is
the one gatherer of a project's and its flat-merged deps' runtime shims and
`[system_libs]` libs/frameworks, used by both `ruxen build` and `ruxen test`;
`collect_system_lib_flags` covers the stdlib packages. `mir/residual_abi.rs` is an ABI
**contract** with the C runtime: every signature must match the C function in
`library/`. A stdlib runtime function's ABI is derived from its `.rx` lib
declaration, never listed there.

## Depends on
MIR (`mir::MirProgram`), `target` policy, external tools (`cc`/`clang`/`wasm-ld`/
the platform linker), and the C runtime in `library/`. No upward edges.

## Invariants & gotchas
- Strict downward order: codegen is the bottom of `lexer→…→codegen`; never reach up.
- ABI: everything crosses as `int64_t` except `Float` (C `double`); `&String` is
  `char*`. A `&var T` is the address of the caller's slot for a String or a
  scalar, else the handle's own word (`mir::refmut_abi`). MIR applies the rule
  and emits `RefMut` only for a slot-addressed `T`, so every backend lowers
  `RefMut` to the source's slot address unconditionally (the MIR walk,
  `lower.rs`, marks the source address-taken; Cranelift gives it a stack
  slot; LLVM uses the alloca). Changing a stdlib runtime signature means
  editing its `.rx` declaration + the C side; only a symbol the compiler emits
  itself (and the wasm bootstrap does not declare) goes in `mir/residual_abi.rs`,
  which `tests/it/runtime_abi_derivation.rs` guards. The REPL JIT resolves runtime
  symbols by dlsym and registers only its capture and slot shims.
- **The erased uniform word (B01) — read before touching any float coercion.**
  A value whose static type codegen cannot see lives in a 64-bit *uniform
  word*: a generic-collection slot (`RuxenVec` / the hash+set tables are
  `int64_t` regardless of `T`), an address-taken local's 8-byte stack slot, a
  param/return whose declared `Ty` is opaque, a closure's params/return when
  reached through an erased `Fn` box, and a scalar method home's `self` (typed
  as the FFI shell `Class{"Float"}`). **A `Float` in a uniform word carries its
  IEEE-754 bit pattern, never an `fcvt` numeric conversion** — `abi::Mode::Word`
  (a `Convert` decided under it, or a backend's `coerce_erased_word`), not
  `Mode::Numeric`. `Float32` is its own 32-bit pattern zero-extended into the
  word. Both directions must move together; half the fix is worse than none.
  - The distinction a machine type CANNOT make: a declared `Int` param and an
    erased `T` param are both `i64`. Implicit numeric conversion into a
    *declared* scalar param (`f(3)` where `def f(x: Float)`) must stay `fcvt`.
    So the decision is keyed on the DECLARED Ruxen type via
    `abi::is_erased_word_ty` (in `abi::call_arg_mode` / `assign_mode`, applied
    by `mir::coercion`), never on the machine type. Pin
    `tests/release-e2e/cases/1000_float_erased_word.rx` guards both that line
    (its `run_1004` section) and the erasure side.
  - The CLOSURE half lives in MIR lowering (`mir/lower/expr/closure.rs` and
    `expr/method_call.rs`, via `FloatToBits` / `BitsToFloat`), deliberately, so
    both backends inherit one implementation instead of each re-deriving it.
- No callee guessing: an abstract `?T_method` callee (receiver still a generic
  parameter) is a hard error in both backends and in `lang_intrinsics`. MIR's
  `prune_unmonomorphised_bodies` guarantees none arrives; never reintroduce a
  "shortest suffix wins" or `T_method` lookup.
- Symbol linkage comes from `MirFunction::linkage` (set by MIR lowering for
  `main` and the `<Mixin>_dynamic_<method>` helpers C calls by symbol); a
  backend never picks it from the function name, and LLVM never imports Cranelift.
- Link order has one owner: `object::link_command_args` (used by
  `emit_executable`, so by `link_host` and `compile_no_std`). It
  puts the program object, runtime objects and the runtime archive (whole-archive,
  or `-force_load` on macOS) first, then every `-l` and link flag. Linkers that
  default to `--as-needed` (Ubuntu) drop a library listed before its user.
- Cranelift is hard-blocked for wasm/embedded (`requires_llvm_backend()`); those
  targets select LLVM (`Backend::select`; `--backend=cranelift` there is an error).
- The wasm path (`compile_cross` → `target.is_wasm()`) now compiles a curated
  runtime subset (`object::WASM_RUNTIME_CORE`: alloc.c, vec.c) + a bundled
  allocator/libc shim (`object::WASM_RT_C`) via clang `--target=wasm32` and links
  them (`emit_wasm_module` takes runtime objects). The vec ABI is derived
  from the array `.rx` declarations (`AbiClass::of` keeps a generic `T` slot at
  i64 on wasm32). Tier 4.09 (WIP) — see
  `docs/requirements/tier4_09_wasm_heap_and_host_imports.md`. KNOWN-OPEN: on wasm32
  the call-site arg lowering passes an `Int` to `ruxen_vec_push` as i32 instead of
  i64 (slot-width bug) → traps; fix is in the call-site arg lowering, not the decl table.
