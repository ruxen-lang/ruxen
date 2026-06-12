# ADR: no_std core split + WASM target (tiers 4.04 + 4.03)

Status: Accepted (2026-06-12)
Branch: `feat/drop-elaboration`
Specs: `docs/requirements/tier4_04_no_std_embedded.md`,
`docs/requirements/tier4_03_wasm_target.md`,
build plan `docs/prompts/v1/16_phase4_no_std_wasm.md`

## Context

Both specs were written against the **pre-restructure** tree:
`crates/ruxen-core/runtime/runtime.c` (a single 426-line libc-coupled blob)
and `share/ruxen/std/`. Neither exists today. The real tree is:

- `compiler/ruxen_core/` (not `crates/ruxen-core/`).
- A **package-split stdlib** at `library/std/<pkg>/`, each package owning its
  own `src/lib.rx` + `runtime/*.c`. There is already a `library/std/core`
  package (`std-core`).
- `-lc -lm` are **no longer hardcoded** in `codegen/object.rs`. They were moved
  (spec "B3" of `zero_rust_stdlib_classes.spec.md`) into
  `library/std/core/Ruxen.toml` `[system_libs] libs = ["c", "m"]`, aggregated
  by `codegen::collect_system_lib_flags()`. The no_std premise in the specs
  ("unconditional `-lc -lm` you can't drop") is therefore already obsolete:
  dropping libc is a *data*/aggregation decision, not a compiler-branch edit.

Per the umbrella scope decision, this pass drives at the two non-slip bars only
(definition-of-done) and files the long tail:

- **Bar A (no_std, T4.04):** a no_std binary builds + runs on host; E1400
  enforces heap-allocation-in-no_std.
- **Bar B (WASM, T4.03):** a `.wasm` built by `ruxen --target
  wasm32-unknown-unknown` runs in node with an asserted result.

## Decisions

### 1. "core" = the existing `library/std/core` package set, not a new `library/core/`

The spec (§5.6) proposes a fresh `share/ruxen/std/core/*` source tree. The
package split already did that partition: `library/std/core` (`std-core`) is
the heap-free primitive surface (the mixin/trait declarations + the low-level C
runtime). Creating a parallel `library/core/` tree would be churn for symmetry's
sake with no bootstrap benefit. **Delta from spec §5.6 / §6:** the `core`
namespace is the existing `std-core` package loaded alone, and the `core` ⊂
`std` re-export elegance (the full `std.core.*` re-export surface) is **filed**,
not built in this pass.

### 2. `no-std` is a manifest key + linker-line consequence, abort-only panic

- `[package] no-std = true` (manifest) is the v1 switch. The package-level
  source directive (`no_std` at top of `main.rx`) is **filed** as the natural
  follow-up — the directive parser work (`panic_handler` / `global_allocator` /
  `no_mangle`) is the staged remainder, not a non-slip bar.
- no_std drops the std `[system_libs]` aggregation and adds `-nostdlib`. Because
  link deps are already per-package data, "drop libc" means "do not aggregate
  the libc-bearing packages' `[system_libs]`," not a hardcoded-flag edit.
- **Panic = abort-only** (spec Open Decision #5). Today `ruxen_panic`
  (`library/std/core/runtime/alloc.c`) does `fprintf(stderr) + exit(101)` —
  both libc. The no_std core path uses an abort-only `ruxen_panic` with no libc
  I/O (`__builtin_trap()` / a write-syscall-free abort). `panic = "unwind"` is
  out of v1 (parses-and-errors is filed).

### 3. E1400 — heap allocation in a no_std unit

Reserved by the build plan (prompt 16). Emitted when a no_std compilation unit
constructs a heap type (`Array` / `String` / `Map` / `Set` / a class with a
heap field — whatever routes through `ruxen_alloc`). Registered in
`diagnostics::codes::REGISTRY`; long-form `docs/errors/E1400.md`. (E1401 `std.*`
import in no_std, E1402 unsupported triple: **reserved, filed** — they ride the
core/std resolver split, which is staged remainder.)

### 4. WASM exports via the LLVM `export_name` function attribute

Decouples the wasm export name from Ruxen name-mangling (spec §9 Q6's stated
preference). Mechanism proven end-to-end before implementation: a C analog with
`__attribute__((export_name("add")))` compiled by clang-18
`--target=wasm32-unknown-unknown -nostdlib`, linked by `wasm-ld --no-entry
--export-dynamic`, validated and called in node (`add(2,3) == 5`). On the
inkwell side: `context.create_string_attribute("export_name", name)` +
`fn_value.add_attribute(AttributeLoc::Function, attr)`. **Not** the
`--export=<mangled>` linker-flag route (spec §5.6 lists both; we pick the
attribute for name stability).

### 5. WASM linker = `wasm-ld` at the LLVM-18 prefix, no libc, no cc

`wasm32-unknown-unknown` is a no_std target with (eventually) an allocator. For
the headline bar — stdout-free math exports — there is **no C runtime and no
libc**: the user object alone links via `wasm-ld --no-entry --export-dynamic`.
The Ruxen LLVM backend already emits the wasm object for the wasm32 triple
(`llvm/mod.rs` threads the cross triple into the `TargetMachine`); the
cross-compile work (tier 4.02) already routes wasm32 to the LLVM backend (CLI
`build.rs:806`, §5.8 guard `codegen/mod.rs:659`). The remaining gap is the
linker branch (cc → wasm-ld) and the export-attribute emission. A bundled
`dlmalloc` allocator + `std.wasm` import module are **filed**.

## LLVM verification lane — bring-up findings

The codegen `CLAUDE.md`'s "NEVER built — no llvm-config" note was **stale**.
LLVM 18.1.8 is installed (`/opt/homebrew/opt/llvm@18`); `wasm-ld` 18.1.8 is at
the same prefix; node v22 is on PATH. With
`LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18`,
`cargo build -p ruxen_core --features llvm` succeeded in ~8s. The **only**
bit-rot was one dead-import warning (`cmpop_to_intpred`, `emit_binop` re-imported
but unused in `llvm/emit/mod.rs`); removed. No inkwell/LLVM API drift. The
default toolchain build stays Cranelift-only; the llvm lane is an additional,
env-gated verification lane.

## Micro-calls (recorded as they happen)

- (Stage 0) Stale "no llvm-config / never built" notes corrected in
  `codegen/CLAUDE.md`, `codegen/llvm/CLAUDE.md`, `codegen/llvm/emit/CLAUDE.md`
  to document the `LLVM_SYS_180_PREFIX` lane.
- (Stage B) **Env gap, not bit-rot:** linking the `--features llvm` *test*
  binary needs `-L /opt/homebrew/opt/zstd/lib` on `RUSTFLAGS` — llvm-sys emits
  `-lzstd` (LLVM 18 was built against zstd) without adding the search path.
  Recorded in the gate invocation; not a code change.
- (Stage B) **Export surface = every top-level free `def`, by source name**
  (not visibility-gated). Ruxen has no `pub` at file scope (top-level `def` is
  `Private`; `main` is special-cased by name), so a visibility gate would
  export nothing. A wasm32 reactor's free-fn set IS its API. The per-function
  `wasm_export "custom"` opt-in/rename directive is filed.
- (Stage B) **Export attribute key = `"wasm-export-name"`** (the LLVM-IR
  spelling clang's `__attribute__((export_name))` lowers to), set via
  `context.create_string_attribute(...)` + `AttributeLoc::Function`. Verified
  end-to-end (node sees the export, returns the right value).
- (Stage B) **wasm build skips the stdlib bootstrap** in `ruxenc::compile`'s
  cross path (`target.is_wasm()` → empty bootstrap). This is the no_std
  reality and the reason the LLVM backend's vtable-globals gap doesn't bite.
- (Stage B) **Latent bug fixed:** `ruxen_cli`'s `llvm` feature now also
  enables `ruxenc/llvm` — otherwise the `Backend::Llvm` variant exists (via
  `ruxen_core/llvm`) while `ruxenc`'s `#[cfg(feature="llvm")]` match arm is
  cfg'd out → E0004 non-exhaustive. Only surfaced now because the wasm bar is
  the first time the CLI is built with llvm.
- (Stage B) **Linker = `wasm-ld`**, discovered via `RUXEN_WASM_LD` → LLVM-18
  prefix → PATH; `--no-entry --export-dynamic --allow-undefined`; no C runtime,
  no `[system_libs]`. Missing linker → actionable error.
- (Stage A) **no_std surface = a `--no-std` CLI flag**, not the source directive
  or manifest key (both filed). Lowest-churn for the single-file `compile` path
  and directly testable; the directive parser is the staged remainder.
- (Stage A) **Two hosted-only injections had to be suppressed for no_std to
  link+run:** (1) the cranelift `ruxen_env_init(argc,argv)` call injected into
  `main` (gated on `!program.no_std` via a `CodeGen.no_std` field set from
  `MirProgram::no_std`), and (2) `synthesize_primitive_fmt_displays`
  (`Int_fmt`/`Char_fmt`/… reference `ruxen_int_to_string`/`Formatter_write_str`)
  — gated on `!self.no_std` via `Lowerer::new_no_std`. Both default false, so
  every hosted build is byte-unchanged.
- (Stage A) **macOS cannot produce a libc-free executable** (`ld: dynamic
  executables must link libSystem.dylib` — verified). So the macOS no_std bar is
  "no Ruxen stdlib + runs + signals a value via a minimal libc `exit` FFI"
  (exit 42, zero `ruxen_*` symbols). The strict `-nostdlib`, zero-undefined
  binary is a Linux target — verified standalone in a `linux/arm64` container
  (`-nostdlib -static` + a `_start` shim + raw `SYS_exit` → exit 42, zero
  undefined symbols) — and is FILED as the embedded/Linux remainder.
- (Stage A) **E1400 detection altitude = typed HIR** (`src/no_std.rs`): a
  recursive expr walk flags the unambiguous allocation kinds (string/array/map
  literals, interpolation, array-fill, collection macros) plus any call whose
  result type is a heap collection. Borrows/reads/pass-throughs are allowed.
  E1400 sits in a new E1400-E1499 namespace (the prompt reserved it); E1401/
  E1402 reserved (not registered until consumed, per the registry rule).
- (Stage A) **`compile_no_std` links the user object alone** (empty runtime
  objects, empty link flags) = bare `cc <obj> -o <out>`. The macOS `-framework
  Security` from `linker_args` remains (harmless — not libc; the binary never
  calls it); dropping it cleanly is a trivial follow-up, filed.
