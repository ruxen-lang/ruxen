# Tier 4.03 — WASM Target

## 1. Summary & Motivation

WebAssembly is the single target with the largest payoff-to-effort ratio in tier 4. "Write Riven, run in browsers / edge workers / Lambda@Edge / serverless / plugins" is a capability unlock no other target provides. Because LLVM 18 (already linked — `crates/riven-core/Cargo.toml:27`) includes a mature `wasm32` backend, the bulk of the code-generation work is already done. What's missing is the *scaffolding*: accepting the triple (tier 4.02), producing a valid `.wasm` module (rather than an ELF executable), shipping a runtime that doesn't assume libc, exposing user functions as wasm *exports*, and importing host functions as wasm *imports*.

This document specifies two levels of WASM support:

- **`wasm32-unknown-unknown`** — minimum viable WASM. No libc, no syscalls, no filesystem. Pure computation. Suitable for web and plugin embedding.
- **`wasm32-wasi`** — WASI Preview 1 support. The `runtime.c` largely compiles as-is against `wasi-libc`, so `std::fs`, `std::io::stdin`, `std::env::args` work.

The Component Model + WIT (WebAssembly Interface Types) story is explicitly **future work** — a paragraph here and a pointer, but no implementation budget in tier 4.

## 2. Current State

### 2.1 No target infrastructure

Doc 02 §2 documents the absence of `--target`. A fortiori there is no wasm32 support.

### 2.2 Linker assumptions (`crates/riven-core/src/codegen/object.rs:52-92`)

```rust
let mut cmd = Command::new("cc");
cmd.arg(&obj_path).arg(runtime_o).arg("-o").arg(output_path)
   .arg("-lc").arg("-lm");
```

For `wasm32-unknown-unknown`:
- `cc` cannot produce a `.wasm`.
- `-lc` does not exist — there is no libc on bare wasm32.
- `-lm` does not exist.

For `wasm32-wasi`:
- `cc` invoking a standard toolchain produces ELF; we need `wasi-sdk`'s `clang` with `--target=wasm32-wasi`.
- `-lc` is provided by `wasi-libc`, but supplied as `libc.a` inside the sysroot rather than auto-linked.

### 2.3 Runtime assumptions (`crates/riven-core/runtime/runtime.c`, 426 lines)

- `#include <stdio.h>` → `fprintf`, `fputs`, `fflush`. Not available in `wasm32-unknown-unknown`.
- `#include <stdlib.h>` → `malloc`, `free`, `realloc`, `abort`. Not available.
- `fprintf(stderr, ...)` in `riven_panic` (line 423-426). Not available.
- Every heap allocation goes through `riven_alloc` → `malloc`. Not available.

For `wasm32-wasi`, `wasi-libc` provides `stdio.h`, `stdlib.h`, `string.h` so the runtime *mostly* works. `stderr` is file descriptor 2 by WASI convention; `fprintf(stderr, …)` does the right thing.

For `wasm32-unknown-unknown`, we need either (a) a tiny allocator inside the runtime (e.g. bump or `dlmalloc`), (b) to go no-alloc (impossible for `Vec`, `String`), or (c) to import an `alloc`/`free` pair from the host.

### 2.4 Entry points

Current: `main()` at `codegen/cranelift.rs` / LLVM emit. WASM has two entry patterns:

- **Reactor** (`wasm32-unknown-unknown`): no `main`; the module exports functions the host calls directly. A WASM reactor has no lifetime — every export is a discrete call.
- **Command** (`wasm32-wasi`): a `_start` function that WASI runtimes (Wasmtime, Wasmer, browsers with a WASI polyfill) invoke. Plays the role of `main()`.

The current compiler emits a function literally called `main`, which is not a reserved wasm export. For `wasm32-wasi` we must rename it to `_start` (the WASI convention).

### 2.5 Tier-1 B5 tension (string literals)

`tier1_00_roadmap.md` B5 documents that string literals flow into `Ty::String` slots as raw pointers. On wasm32, pointers are 32-bit (unlike the 64-bit `i64` slot used in the int64-erased runtime). Every place that currently assumes `sizeof(void*) == sizeof(int64_t)` must be triaged before wasm32-unknown-unknown works.

Specifically, `runtime.c:220-225` (Vec stores `int64_t` elements per tier-1 §01 2.1):

```c
typedef struct {
    int64_t* data;
    size_t len;
    size_t cap;
} RivenVec;
```

`int64_t` on wasm32 is still 64 bits — wasm32 means 32-bit *pointers*, not 32-bit integers. So Vec-of-int64 works. But `Vec<String>` stores `char*` pointers in 64-bit slots; on wasm32, half of each slot is padding. That wastes memory and breaks any runtime code that casts `int64_t` → pointer assuming the pointer occupies the low bits (endianness trap on big-endian wasm, though wasm is LE).

Recommendation: introduce `riven_slot_t` in the runtime, `typedef` to `intptr_t`. It's `int32_t` on wasm32 and `int64_t` on 64-bit targets. Tier-1 §10 R5 already flags this.

## 3. Goals & Non-Goals

### Goals

1. `riven build --target wasm32-unknown-unknown` produces a valid `.wasm` module.
2. `riven build --target wasm32-wasi` produces a WASI-compatible `.wasm` executable runnable under `wasmtime run <file>.wasm`.
3. `@[wasm_export("name")]` on a `pub def` exports it to the host.
4. `@[wasm_import("module", "name")]` in an `extern "C"` block imports a host function.
5. A minimal in-runtime allocator for `wasm32-unknown-unknown` (bump or `dlmalloc`).
6. `std::io::println` works on WASI (writes to fd 1 via `fd_write`).
7. Example in `examples/04-wasm-hello/` (doc 07) demonstrating the full pipeline + an HTML harness.
8. `slot_t` plumbing so `sizeof(riven_slot_t) == sizeof(void*)`.

### Non-Goals

- **The Component Model.** Full WIT interface types, `cargo-component`-equivalent. Significant design work — out of v1.
- **`wasm64`.** Real 64-bit linear memory is shipping but unstable across runtimes.
- **Threads (`wasm32-unknown-unknown+threads`).** Shared memory + atomics. Ties to tier-1 concurrency. Out of scope.
- **`wasm32-wasip2`** (WASI 0.2 / preview2). Stabilizing; revisit once Wasmtime LTS supports it.
- **SIMD.** LLVM will emit SIMD ops if the source uses them; we don't expose an explicit knob.
- **DOM bindings / `wasm-bindgen` equivalent.** That's a separate library, not a compiler concern.
- **WASI HTTP / sockets.** Fine when they stabilize, out of v1.
- **`wasm-opt` integration.** Users can post-process manually.

## 4. Surface

### 4.1 CLI

```
riven build --target wasm32-unknown-unknown [--release]
riven build --target wasm32-wasi [--release]
```

Output: `target/wasm32-<env>/<profile>/<pkg-name>.wasm`.

### 4.2 Source annotations

**Exports** (wasm32-unknown-unknown or wasi):

```riven
@[wasm_export("add")]
pub def add(a: Int32, b: Int32) -> Int32
  a + b
end

@[wasm_export]                              # name defaults to the function's name
pub def greet(name: *UInt8, len: USize) -> *UInt8
  # ... returns a pointer the caller must free via alloc/free imports
end
```

**Imports** (host-provided):

```riven
@[wasm_import("env", "console_log")]
extern "wasm"                               # new ABI string; "C" is the existing one
  def console_log(ptr: *UInt8, len: USize)
end

@[wasm_import("env", "now_ms")]
extern "wasm"
  def now_ms -> Int64
end
```

For `wasm32-wasi`, the WASI imports are *implicit* and come from a curated set in `std::wasi::*` (new hidden module). The user does not manually write WASI imports; `std::io::println`, `std::fs::read_to_string`, `std::env::args` use them internally.

### 4.3 Manifest

```toml
[target.wasm32-unknown-unknown]
linker = "wasm-ld"                          # default
# or: linker = "lld", link-args = ["-flavor", "wasm"]
link-args = ["--no-entry", "--export-dynamic", "-O2"]

[target.wasm32-unknown-unknown.dependencies]
# Wasm-specific deps only

[target.wasm32-wasi]
linker = "wasm-ld"
link-args = []
```

### 4.4 Runtime layout

```
~/.riven/lib/runtime/
├── wasm32-unknown-unknown/
│   ├── runtime.o              # Compiled from runtime_wasm.c with bump allocator
│   └── version
└── wasm32-wasi/
    ├── runtime.o              # Compiled from runtime.c against wasi-libc
    └── version
```

## 5. Architecture / Design

### 5.1 Backend selection

wasm32 requires LLVM. Auto-switch the backend in `build.rs:277-288` when the triple's architecture is `Wasm32`, even for debug builds. Emit a note the first time this happens: `"note: wasm32 target requires LLVM backend; using LLVM for debug profile"`.

### 5.2 Module emission (LLVM side)

LLVM 18's wasm32 backend emits a WebAssembly object (a `.wasm` with a `reloc` section). `wasm-ld` links the object(s) into a final `.wasm`.

In `codegen/llvm/mod.rs:47-61`, the `CodeType` passed to `create_target_machine` is already `CodeModel::Default`; for wasm that's correct. Set:

- `RelocMode::PIC` (existing) — required for wasm.
- `--export=<name>` linker args per `@[wasm_export]`. Collected in the MIR program (new `MirProgram::wasm_exports: Vec<String>` field).
- `--no-entry` for `wasm32-unknown-unknown` (no `_start` — it's a reactor module).
- `--export-dynamic` to keep exports from being stripped.
- `--import-memory` (optional — discussion §9).

### 5.3 Entry-point handling

- `wasm32-unknown-unknown`: no entry. `main()` is skipped/renamed/unreferenced.
- `wasm32-wasi`: rename `main` to `_start`. WASI runtimes look up `_start` and invoke it with no arguments; `_start` internally calls `__wasi_args_sizes_get` / `__wasi_args_get` to build argv (handled by `wasi-libc`'s `__main_void` wrapper).

Easiest path for `wasm32-wasi`: link with `wasi-libc`'s crt0 and let it provide `_start`, which calls the user's `main`. No compiler change needed.

For `wasm32-unknown-unknown`: the current codegen emits a `main` function symbol; on wasm linker, with `--no-entry` and no `_start`, this is harmless — `main` becomes an unused internal function. Users that want their `main` to be callable add `@[wasm_export("main")]`.

### 5.4 Runtime for `wasm32-unknown-unknown`

Fork `runtime.c` into `runtime_wasm.c` (or a `#ifdef __wasi__`/`#ifdef __wasm__` maze — discussion §9).

Replacements needed:

| Function | Host runtime.c | wasm32-unknown-unknown runtime_wasm.c |
|---|---|---|
| `malloc`/`free`/`realloc` | libc | Bundled `dlmalloc` (50KB of C source, BSD-licensed) OR a bump allocator |
| `fprintf(stderr, …)` | libc | Imported `env.console_log(ptr, len)` |
| `abort()` | libc | `__builtin_trap()` (compiles to `unreachable`) |
| `strlen`, `memcpy`, `memset` | libc | compiler-rt / Clang builtins (automatic when compiled with `--target=wasm32-unknown-unknown`) |
| `printf` family | libc | Drop — only used indirectly; replace with explicit byte-length functions |

Recommendation: ship `dlmalloc` (well-tested, used by Rust's wasm32 allocator). ~2KB of wasm binary size. Users can opt into a smaller `wee_alloc`-style allocator later. Bump-only is too restrictive for `Vec.pop` + reallocation scenarios.

### 5.5 Runtime for `wasm32-wasi`

The existing `runtime.c` compiles against `wasi-libc` without modification *provided* we drop the implicit `-lc -lm` (wasi-sdk's clang auto-adds them via the sysroot) and use `wasi-sdk`'s `clang --target=wasm32-wasi` as the compiler.

Build recipe for the runtime:

```bash
$WASI_SDK_PATH/bin/clang --target=wasm32-wasi \
    -c crates/riven-core/runtime/runtime.c \
    -o ~/.riven/lib/runtime/wasm32-wasi/runtime.o
```

`argv`: `wasi-libc` fills it via `__wasi_args_get`; the existing tier-1 plumbing (`riven_env_init(argc, argv)` in tier1_01_stdlib.md §7.6) works unchanged.

### 5.6 `@[wasm_export]` / `@[wasm_import]` lowering

- Parser: extend `parse_attributes` (`parser/mod.rs:1572-1610`) to accept `@[wasm_export("name")]` and `@[wasm_import("module", "name")]`. Keep shape compatible with the existing attribute machinery.
- Typeck / HIR: annotate the `HirFunction` with `export_name: Option<String>` and `import_source: Option<(String, String)>`.
- MIR: propagate. `MirProgram` grows `pub wasm_exports: Vec<(String, String)>` (Riven symbol, wasm export name).
- LLVM emit: for exports, set `LLVMSetLinkage(fn, LLVMExternalLinkage)` + `LLVMSetDLLStorageClass` (the wasm attribute equivalent) and emit `wasm.custom.attributes = ["used"]`. For imports, declare the function with `LLVMLinkageExternalWeak` and attach the `wasm-import-module` + `wasm-import-name` attributes.
- Linker: `--export=<riven-mangled-name>` is the primary mechanism. LLVM attribute-based export also works and is preferred for name stability (we can emit the *wasm export name* and the *LLVM function name* independently, avoiding Riven-name-mangling leakage).

### 5.7 Memory model

- `wasm32-unknown-unknown`: default 1 page (64KB) of linear memory, growable via `memory.grow`. Exported as `memory` by the linker default. Users can request `--import-memory` via `link-args` for advanced embedding (share memory with host).
- `wasm32-wasi`: same, plus WASI imports for fs/io.
- Stack: wasm32-ld defaults to 1MB stack. Sufficient for v1.

### 5.8 `std::wasm` module (host API surface)

New hidden module. Users don't write `@[wasm_import]` directly; they call:

```riven
use std::wasm
std::wasm::debug_log("hello")                 # wraps @[wasm_import("env", "debug_log")]
std::wasm::memory_size_bytes() -> USize
std::wasm::memory_grow_pages(n: USize) -> USize
```

Each entry is a thin Riven wrapper over an `extern "wasm"` declaration. Users that want ad-hoc imports do it at the FFI level.

### 5.9 `slot_t` plumbing (tier-1 pre-work)

In `runtime.c`, replace `int64_t` slots with `intptr_t` throughout Vec/Hash/Set/`riven_vec_push`/etc. This is a ~50-line churn. Required before wasm32 vector operations are correct; compatible with host 64-bit as `intptr_t` == `int64_t` there.

This is tier-1 work (flagged there under tier1_00 §10 R5). If tier 4.03 ships before tier-1 resolves R5, we carry the `int64_t`-only wasm32 restriction and type-error any `Vec.map` that would store a pointer-sized element.

### 5.10 WIT / Component Model (future work)

The Component Model adds type-safe WASM ABI definitions via WIT. Tooling:

- `wit-bindgen` emits host/guest bindings from `.wit` files.
- `wasm-tools component new` wraps a core wasm module into a component.

For Riven this would mean:

- A `[wit]` table in `Riven.toml` pointing at `.wit` files.
- A `riven wit bindgen` subcommand that generates Riven-side stubs.
- `@[wit_export("interface-name", "method")]` attributes.

Explicitly deferred. Call out in the book that the Component Model will be Riven's v2 story for WASM ABI stability.

## 6. Implementation Plan — files to touch

### New files

- `crates/riven-core/runtime/runtime_wasm.c` — the `wasm32-unknown-unknown` subset with bundled `dlmalloc` and import-based `console_log`.
- `crates/riven-core/runtime/dlmalloc.c` + `dlmalloc.h` — vendored from the canonical `ftp.gnu.org/…/dlmalloc.c` (BSD-0 license, public domain).
- `crates/riven-core/src/codegen/wasm.rs` — wasm-specific codegen helpers: export/import attribute emission, linker invocation.
- `share/riven/std/wasm.rvn` — `std::wasm` module (exposed once stdlib-as-source lands per tier 1 §7.8).
- `share/riven/std/wasi/` — `std::wasi` hidden module with `fd_write`, `args_get`, etc., imports.
- `examples/04-wasm-hello/` — see doc 07.

### Touched files

- `crates/riven-core/src/parser/mod.rs:1572-1610` — attribute parser: accept `wasm_export`, `wasm_import`.
- `crates/riven-core/src/parser/ast.rs:770-` — `LinkAttr` etc. grow a `WasmImport` / `WasmExport` variant.
- `crates/riven-core/src/hir/nodes.rs` — `HirFunction` gains `wasm_export_name: Option<String>`, `wasm_import: Option<(String, String)>`.
- `crates/riven-core/src/mir/nodes.rs:19-26` — `MirProgram` gains `wasm_exports: Vec<(String, String)>`, `wasm_imports: Vec<FfiFuncDecl>` (wasm-flavored).
- `crates/riven-core/src/codegen/llvm/emit.rs` — emit wasm attributes on imports/exports.
- `crates/riven-core/src/codegen/object.rs:52-92` — wasm-target branch drops `-lc -lm`, invokes `wasm-ld` with `--export`/`--import-memory`/etc.
- `crates/riven-core/src/codegen/mod.rs:27-65` — `find_runtime_obj` (doc 02 §5.6) resolves `wasm32-*` to `runtime_wasm.o`.
- `crates/riven-core/runtime/runtime.c` — the `int64_t` → `intptr_t` slot churn (tier-1 pre-work).
- `crates/riven-cli/src/cli.rs` — no direct change; `--target` (doc 02) covers it.
- `.github/workflows/release.yml` — add wasm runtime artifacts.
- `.github/workflows/ci.yml` (doc 06) — wasm matrix entry: `wasmtime run target/wasm32-wasi/debug/hello.wasm` smoke test.

### Tests

- `crates/riven-core/tests/wasm_codegen.rs` — gated on `cfg(feature = "llvm")`. Compiles a trivial program for `wasm32-unknown-unknown`, loads the `.wasm` via `wasmi` (small, pure-Rust wasm interpreter), invokes an exported function, asserts the return value.
- `crates/riven-core/tests/wasi_codegen.rs` — same but for `wasm32-wasi`, using `wasmi_wasi` or shelling out to `wasmtime`.
- Example `examples/04-wasm-hello/` doubles as an end-to-end acceptance test.

## 7. Interactions with Other Tiers

- **Tier 4.02 cross-compilation.** Hard dependency. Every piece of this doc assumes `--target` works.
- **Tier 4.04 no_std.** `wasm32-unknown-unknown` is essentially a no_std target with an allocator. The `runtime_wasm.c` is very close to what doc 04's `runtime_core.c` would be — worth unifying.
- **Tier 1 stdlib.** `std::io`, `std::env`, `std::fs` on wasi are wrappers over WASI syscalls (via `wasi-libc`). The existing `runtime.c` entries for `fopen`/`fread`/`fwrite` work unmodified. On `wasm32-unknown-unknown`, these modules are `@[cfg(not(target_arch = "wasm32"))]` gated and unavailable.
- **Tier 1 concurrency.** Not supported on wasm32 in v1. `std::thread::spawn` is `@[cfg(not(target_arch = "wasm32"))]`. Raising this restriction requires the WASI threads proposal + shared-memory support.
- **Tier 1 async.** Single-threaded `block_on` (tier1_03 §8 phase 3c) is fine on wasm. Async I/O via WASI pollables is a future integration.
- **Tier 4.05 cbindgen.** Irrelevant for wasm — cbindgen emits C headers, wasm consumers use WIT or raw imports.
- **Tier 4.06 CI.** A `wasm32-wasi` matrix entry that runs `wasmtime run` on the compiled artifact is the strongest possible smoke test.

## 8. Phasing

### Phase 3a — wasm32-wasi MVP (1 week, after doc 02 phase 2c)

1. `--target wasm32-wasi` routes to LLVM.
2. Linker invocation: `$WASI_SDK_PATH/bin/wasm-ld --target=wasm32-wasi …` with the user's `.o` + `runtime.o` + `wasi-libc`.
3. Runtime: compile `runtime.c` against `wasi-libc`, cache as `runtime.o`.
4. `_start` wired (via `wasi-libc`'s crt0).
5. `slot_t` churn in `runtime.c`.
6. **Exit:** a hello-world Riven program builds to a `.wasm` file that `wasmtime run hello.wasm` prints "Hello, Riven!" to stdout. (Requires tier-1 §7.6 argv shim.)

### Phase 3b — wasm32-unknown-unknown MVP (2 weeks)

1. `runtime_wasm.c` with `dlmalloc` + imported `debug_log`.
2. `@[wasm_export]` / `@[wasm_import]` attribute parsing + lowering.
3. LLVM emit: attach wasm-specific attributes.
4. `wasm-ld --no-entry --export-dynamic …` invocation.
5. **Exit:** a Riven program with `@[wasm_export("fib")] pub def fib(n: Int32) -> Int32` produces a `.wasm` that the example HTML page in `examples/04-wasm-hello/` loads via `WebAssembly.instantiate` and invokes from JS, returning the correct value.

### Phase 3c — std::wasm module (0.5 week)

1. `std::wasm::debug_log`, `memory_size_bytes`, `memory_grow_pages`.
2. `@[cfg(target_arch = "wasm32")]` gating of wasm-only stdlib items.
3. **Exit:** `use std::wasm; std::wasm::debug_log("hi")` works in a wasm32-unknown-unknown build.

### Phase 3d — CI + examples (0.5 week)

1. CI matrix entry for `wasm32-wasi` + `wasmtime run` smoke test.
2. `examples/04-wasm-hello/` with a working HTML harness.
3. **Exit:** CI is green; `README.md` links to the example.

### Phase 4 — future work (not in scope)

- WIT + Component Model.
- `wasm32-unknown-unknown+threads`.
- `wasm-opt` integration.
- WASI Preview 2 / `wasm32-wasip2`.
- WASI HTTP / sockets.
- `wasi-nn`.

## 9. Open Questions & Risks

1. **`dlmalloc` vs bump vs host-imported allocator.** Recommend `dlmalloc` — it's the battle-tested choice Rust's wasm32 target uses. Bump is too restrictive (no reallocation). Host-imported is elegant but fragmenting (every host embedding must provide `alloc`/`free`).
2. **Runtime `.c` fork vs `#ifdef` soup.** Two files (`runtime.c`, `runtime_wasm.c`) keep each simple but duplicate some code. One file with `#if defined(__wasm__) && !defined(__wasi__)` is a maintenance cliff. Recommend: keep them split, factor the truly-shared parts (string ops, Vec structural code) into `runtime_common.h`.
3. **`wasi-sdk` dependency.** Building `runtime.o` for wasi requires the user to have `wasi-sdk` installed. Recommend: `riven target add wasm32-wasi` ships a precompiled runtime. No wasi-sdk required for *using* the target, only for developing the runtime.
4. **`wasm-ld` availability.** On macOS with `rustup` installed, `rust-lld` (which is `wasm-ld` under the hood) lives at `~/.rustup/toolchains/<version>/lib/rustlib/<triple>/bin/rust-lld`. `lld` from Homebrew works. Distros ship `lld` as an apt/dnf package. Recommend: detect one, error helpfully if none found (point at `rustup component add rust-src` — no, point at `apt install lld` / `brew install lld`).
5. **Memory ownership across the JS boundary.** If Riven returns a `String` as `(*UInt8, USize)` to JS, who frees it? JS doesn't know the Riven allocator. Recommend: export `riven_alloc` / `riven_free` from the runtime; JS callers are responsible for freeing. Document this as the "you return a pointer → you own the memory → remember to call free" convention, matching `wasm-bindgen`'s approach.
6. **Name mangling.** LLVM mangles function names for debug info. Wasm exports must be exact ASCII. Recommend: use `LLVMAddFunctionAttr(fn, "wasm-export-name", name)` (LLVM attribute) to decouple the LLVM symbol name from the wasm export name — avoids surprising users with mangled names.
7. **Binary size.** Riven's wasm output will be larger than Rust's — our runtime brings a C malloc + stdlib shims. Recommend: document that `--release` + `wasm-opt -Oz` (user-run) brings it in line with Rust; budget ~30KB for "Hello World" on `wasm32-unknown-unknown`.
8. **Debug info.** `wasmtime` has limited DWARF support on wasm. Recommend: emit DWARF in debug builds anyway; it's useful in browsers via the DevTools C/C++ DevTools extension.
9. **Floating-point traps.** Wasm2 SIMD has `f32x4.div`-style traps on NaN in some runtimes. Recommend: document; no action.
10. **Reactor vs command module conflict.** A user might want a wasm32-wasi reactor (no `_start`, only exports). Recommend: support via `@[package.wasm-crate-type = "cdylib"]` manifest key → pass `--no-entry` to `wasm-ld` even on wasi.
11. **Imported memory.** If the host provides memory, Riven's allocator needs to start from a nonzero offset to avoid clobbering the host's data. Recommend: v1 always uses `--export-memory` (Riven owns memory); `--import-memory` deferred.
12. **`intptr_t` on wasm32 is `i32`.** Anywhere the codegen assumes 64-bit slots breaks. Tier-1 R5 (slot erasure) must land before wasm32 has a correct Vec of pointers. Gate wasm32 release-tier behind R5.

## 10. Acceptance Criteria

Phase 3a — wasm32-wasi:

- [ ] `riven build --target wasm32-wasi` on a hello-world produces a valid `.wasm` file (validated with `wasm-validate` if available, else via `wasmtime --help | head`).
- [ ] `wasmtime run target/wasm32-wasi/debug/hello.wasm` prints "Hello, Riven!" to stdout.
- [ ] `wasmtime run --dir=. target/wasm32-wasi/debug/cat.wasm -- README.md` reads a file from the current directory and prints its contents (requires tier-1 `std::fs::read_to_string` and `std::env::args`).
- [ ] Binary size of hello-world on `wasm32-wasi` is ≤ 200KB in `--release`.

Phase 3b — wasm32-unknown-unknown:

- [ ] `riven build --target wasm32-unknown-unknown` on a program with `@[wasm_export("add")] pub def add(a: Int32, b: Int32) -> Int32` produces a `.wasm` whose exports include `add` (validated via `wasm-objdump -x`).
- [ ] Loading that `.wasm` in Node.js via `WebAssembly.instantiate` and calling `instance.exports.add(2, 3)` returns `5`.
- [ ] A program using `std::wasm::debug_log("hello")` compiled for wasm32-unknown-unknown imports `env.console_log` (validated via `wasm-objdump -x`).
- [ ] `@[wasm_import("host", "now_ms")] extern "wasm" def now_ms -> Int64 end` declares an import on module `host` with field `now_ms` of signature `() -> i64`.
- [ ] Binary size of `fib(n)` on `wasm32-unknown-unknown` is ≤ 50KB in `--release`.

Phase 3c — std::wasm:

- [ ] `use std::wasm` works in a wasm32-unknown-unknown build and fails to resolve on x86_64 (`@[cfg(target_arch = "wasm32")]` gating).
- [ ] `std::wasm::memory_size_bytes` returns a multiple of 65536.

Phase 3d — CI + examples:

- [ ] `examples/04-wasm-hello/` has `index.html` + `build.sh` + a `Riven.toml` that targets `wasm32-unknown-unknown`; opening `index.html` in a browser and clicking the button shows the Riven function's output.
- [ ] CI runs `riven build --target wasm32-wasi` and `wasmtime run` on the result, asserts exit code 0 and expected stdout.
