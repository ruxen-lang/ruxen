# Tier 4.04 — no_std / Embedded Mode

> **Implementation note (2026-06-12, tier 4.04 pass).** This spec predates the
> `crates/ruxen-core/` → `compiler/ruxen_core/` move, the per-package stdlib
> split (`library/std/<pkg>/`, with a `library/std/core` package already
> present), and the move of `-lc -lm` out of `codegen/object.rs` into
> per-package `[system_libs]` tables. The spec's central premise — "the link
> line is unconditionally `-lc -lm` and you can't drop it" — is therefore
> already OBSOLETE: dropping libc is a data/aggregation decision, not a
> compiler-branch edit. What landed in this pass (the non-slip bar): a
> `--no-std` host build skips the stdlib bootstrap, links WITHOUT the Ruxen C
> runtime / `[system_libs]` (no `ruxen_*` symbol in the binary), suppresses the
> hosted-only entry plumbing (the `ruxen_env_init` injection + synthesized
> primitive `*_fmt` Display helpers), and **E1400** rejects heap allocation
> (String/Array/Map/Set) in a no_std unit. Bars: `examples/06-no-std/`,
> `scripts/no_std_verify.sh`, pins `tests/no_std_e1400.rs` (+ the registry).
> **"core" = the existing `library/std/core` package set** (ADR decision #1),
> not a new `library/core/` tree. **Platform reality:** on macOS a truly
> libc-free binary is impossible (the OS mandates `libSystem` for any dynamic
> executable); the macOS bar links no Ruxen stdlib and runs, signalling a
> computed value via a minimal libc `exit` FFI. The strict `-nostdlib`,
> zero-libc-imports binary is a Linux/embedded target (a `_start` shim + raw
> exit syscall) and is the **staged remainder** — along with the `core`/`std`
> re-export surface, the `alloc` tier (heap types with a user
> `global_allocator`), the `panic_handler`/`global_allocator`/`no_std` source
> directives, the thumbv7em fixture, and `panic = "unwind"`. See
> `docs/decisions/phase4-no-std-wasm.md`.

## 1. Summary & Motivation

Every Ruxen program today links against a C runtime (`crates/ruxen-core/runtime/runtime.c`, 426 lines) that pulls in libc (`stdio.h`, `stdlib.h`, `string.h`) and assumes a hosted environment. The final binary always links `-lc -lm` (`crates/ruxen-core/src/codegen/object.rs:64-70`). There is no way to turn this off. That eliminates Ruxen from:

- **Bare-metal embedded.** Cortex-M / RISC-V microcontrollers have no libc and 16KB of RAM. You bring your own panic handler, your own allocator (if any), and your own linker script.
- **Kernel development.** No dynamic allocation, no I/O beyond what you expose through MMIO.
- **WASM32-unknown-unknown.** Technically has no libc; doc 03 works around this by shipping `dlmalloc` in a wasm-specific runtime.
- **OS-level components.** Bootloaders, kernel modules, UEFI apps.

This document specifies a `no_std` mode: a compiler switch + manifest key + package-level directive that tells the toolchain "I'm providing the host environment; don't link libc, don't assume `malloc`, don't emit a default `main`, give me hooks for panic and allocation."

This is also prerequisite work for tier 4.03 WASM `wasm32-unknown-unknown`. Doc 03 §4.4 ships `runtime_wasm.c` as a one-off; doing no_std properly unifies that effort.

## 2. Current State

### 2.1 Runtime (`crates/ruxen-core/runtime/runtime.c`)

All 426 lines assume a hosted C environment. Every function either:

- Includes `<stdio.h>` and uses `fputs` / `fprintf` / `fflush`.
- Includes `<stdlib.h>` and uses `malloc` / `free` / `realloc` / `exit` / `abort`.
- Includes `<string.h>` and uses `strlen` / `memcpy` / `strcmp`.

Panic (line 423-426):

```c
void ruxen_panic(const char *msg) {
    fprintf(stderr, "panic: %s\n", msg);
    abort();
}
```

Allocation (line 144-163):

```c
void *ruxen_alloc(size_t size) { return malloc(size); }
void ruxen_dealloc(void *p) { free(p); }
void *ruxen_realloc(void *p, size_t sz) { return realloc(p, sz); }
```

### 2.2 Link line (`crates/ruxen-core/src/codegen/object.rs:64-70`)

```rust
let mut cmd = Command::new("cc");
cmd.arg(&obj_path).arg(runtime_o).arg("-o").arg(output_path)
   .arg("-lc").arg("-lm");
```

Unconditional. No way for the user to drop `-lc -lm` or pass `-nostdlib`.

### 2.3 Entry point

The compiler emits a user `main` (Cranelift: `codegen/cranelift.rs`; LLVM: `codegen/llvm/emit.rs`). The C runtime provides no crt0 — the C startup (`_start` on ELF, `main` on OS/X) comes from libc via `-lc`.

For a no_std build, we need:
- To **not** emit `main` (or: emit it only when hosted).
- To **not** pull libc's crt0 (handled by `-nostdlib`).
- To expose hooks for user-supplied panic and allocation.

### 2.4 Tier-1 bug B1 (Drop is a no-op)

`tier1_00 §B1` documents that `MirInst::Drop` is silently discarded in both codegen backends. In hosted mode this leaks memory until process exit. In no_std / embedded, a 16KB microcontroller exhausts its heap in a few `Array` allocations. **No_std cannot ship without B1 fixed.**

### 2.5 No directive infrastructure for panic handler / allocator

Current directive parser (`parser/mod.rs:1572-1610`) dispatches link options and (aspirationally) `layout` / structural-mixin includes. There is no `panic_handler` directive, no `global_allocator` directive, no `no_mangle` directive. Tier-1 B2 discusses the directive-handling cleanup.

### 2.6 No `cfg(...)` item-gating

Tier-1 B6 reserves `async`/`await`/`spawn`/`actor`/`send`/`receive` keywords that are never consumed. No symmetric reservation for conditional-compilation directives. Doc 01 §5.2 introduces body-level `cfg(feature = …)`; doc 04 extends that to body-level `cfg(target_os = "none")` (the embedded convention) and `cfg(not(feature = "std"))`.

## 3. Goals & Non-Goals

### Goals

1. `[package] no-std = true` manifest key (or equivalently: `[features] default = []` with a non-activating `std`-gate feature, Rust-style).
2. Package-level `no_std` directive (one line at the top of the package root) as an alternative to the manifest key.
3. Body-level `panic_handler` directive on a user function: `def(msg: &PanicInfo) -> !`.
4. Body-level `global_allocator` directive on a user value or class including an `Allocator` mixin.
5. `panic = "abort" | "unwind"` in `[profile.*]`, defaulting to `abort`. **v1 implements `abort` only.**
6. A split runtime: `runtime_core.c` (no libc, no malloc, no I/O) linked in *every* build, plus `runtime_std.c` (the current `runtime.c` minus core bits) linked only when `no-std = false`.
7. A `core` module set under `std.core.*` (re-exported from `std`) that works in no_std builds. Anything `std`-prefixed that touches libc or malloc becomes unavailable.
8. Body-level `no_mangle` directive to export a Ruxen function with its source name (no mangling).
9. Linker-line control: `-nostdlib` added when `no-std = true`; `-lc -lm` removed.
10. Compatibility with tier 4.03: `wasm32-unknown-unknown` with `no-std = true` works without `runtime_wasm.c`'s bundled `dlmalloc` — users ship their own.

### Non-Goals

- Unwinding panics in v1. `panic = "unwind"` parses and errors with "not yet supported."
- An opinionated bundled allocator. We provide the `Allocator` mixin + body-level `global_allocator` directive hook; the user brings their own (analogous to Rust's `#[global_allocator] static A: MyAlloc = MyAlloc;`).
- Embedded-specific stdlib (`embedded-hal`-equivalent). That's downstream ecosystem work.
- A ready-made `cortex-m-rt`-equivalent. Out of scope.
- Hot-patching / live reload. Out of scope.
- Multiple allocators per crate. One `global_allocator` directive per binary, Rust-style.

## 4. Surface

### 4.1 Manifest

```toml
[package]
name = "blinky"
version = "0.1.0"
no-std = true                                               # top-level switch

[profile.release]
opt-level = "z"                                             # optimize for size
lto = true
panic = "abort"                                             # required for no-std v1

[target.thumbv7em-none-eabihf]
linker = "arm-none-eabi-ld"
link-args = ["-T", "memory.ld", "-T", "link.ld", "--gc-sections"]
```

### 4.2 Source directives

**Package-level** (top of `src/lib.rx` or `src/main.rx`):

```ruxen
no_std                                                      # alternative to [package] no-std = true

use core.Option
use core.Result
use core.panic.PanicInfo
```

**Panic handler** (required in no_std binaries):

```ruxen
def my_panic(info: &PanicInfo) -> !
  panic_handler
  # ... do whatever makes sense for the target ...
  loop; end
end
```

**Global allocator** (required for no_std if `Array`/`String`/`Map`/`Set` are used):

```ruxen
class MyAllocator
  def alloc(size: USize, align: USize) -> *var UInt8
    # ... MMIO or user-provided allocator ...
  end
  def dealloc(ptr: *var UInt8, size: USize, align: USize)
    # ... ...
  end
end

let ALLOCATOR: MyAllocator = MyAllocator.new
global_allocator :ALLOCATOR
```

**No-mangle** (for exporting a C-callable function):

```ruxen
def app_main
  no_mangle
  # ... called from assembly or a crt0 you wrote ...
end
```

### 4.3 `core` namespace

The stdlib is bisected. Items available in `core` (no-alloc, no-I/O, no-libc):

- Primitives: `Int*`, `UInt*`, `Float*`, `Bool`, `Char`, `USize`, `ISize`.
- `Option`, `Result`.
- `Range`, `RangeInclusive`.
- Mixins: `Copy`, `Clone`, `Drop`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`, `Default`, `From`, `Into`, `TryFrom`, `TryInto`, `Iterator`, `IntoIterator`, `FromIterator`.
- `mem.size_of`, `mem.align_of`, `mem.replace`, `mem.swap`, `mem.take`.
- `ptr.null`, `ptr.null_mut`, `ptr.read`, `ptr.write`, `ptr.read_volatile`, `ptr.write_volatile`.
- `slice.from_raw_parts`, `slice.from_raw_parts_mut`.
- Macros: `panic!`, `assert!`, `assert_eq!`, `todo!`, `unimplemented!`.

Items available only in `std` (hosted):

- `io`, `fs`, `env`, `process`, `net`, `time`, `path`, `hash.DefaultHasher`, `prelude.println!`/`eprintln!`/`format!`/`print!`/`eprint!`.
- `Array[T]`, `String`, `Map[K,V]`, `Set[T]`, `Box[T]` — but see §4.4 below.

### 4.4 `alloc` tier

Following Rust, there's a middle layer: `alloc` — items that need an allocator but not the full hosted environment. Shipped in `core.alloc.*`:

- `Array[T]`, `String`, `Box[T]`, `Map[K,V]`, `Set[T]`.
- Blanket includes for `Iterator`-from-collection.
- `alloc.Layout`, `alloc.Allocator` mixin (the one a `global_allocator`-marked value includes).

To pull these in, a no_std crate does:

```ruxen
no_std
use core.alloc.Array
use core.alloc.Box
```

If no `global_allocator` directive is provided and the user tries to use `Array`, the linker emits an unresolved-symbol error naming `__ruxen_global_allocator`. Sharp but clear.

### 4.5 `PanicInfo`

```ruxen
struct PanicInfo
  # Minimal. No downcasting; message is an owned slice.
  message: &static str
  file: &static str
  line: UInt32
  col: UInt32

  def message -> &str; self.message; end
  def location -> (&str, UInt32, UInt32); (self.file, self.line, self.col); end
end
```

Passed into a `panic_handler`-marked function by value-of-reference. The compiler synthesizes it at every `panic!` call site, threading `file!`/`line!`/`column!` literals.

### 4.6 Allocator mixin

```ruxen
mixin Allocator
  def alloc(layout: Layout) -> Result[*var UInt8, AllocError]
  def dealloc(ptr: *var UInt8, layout: Layout)

  # Optional, with defaults:
  def alloc_zeroed(layout: Layout) -> Result[*var UInt8, AllocError]
  def realloc(ptr: *var UInt8, old: Layout, new: Layout) -> Result[*var UInt8, AllocError]
end

struct Layout
  size: USize
  align: USize

  def self.new(size: USize, align: USize) -> Result[Layout, LayoutError]
  def self.for[T] -> Layout                            # compile-time size_of + align_of
end

struct AllocError
  # Zero-sized marker
end
```

The `global_allocator` directive binds a `static`-lifetime value that includes `Allocator`; `Array`, `String`, etc. route their allocations through it by dispatching on a compiler-internal `__ruxen_global_allocator` symbol.

### 4.7 Linker line

No-std builds drop:

- `-lc`
- `-lm`
- The C crt0 (`crt1.o`, `crti.o`, `crtbegin.o`, `crtend.o`, `crtn.o`).

And add:

- `-nostdlib`
- User-provided link-args from `[target.<triple>].link-args`.

## 5. Architecture / Design

### 5.1 Runtime split

Current `runtime.c`:

```
runtime.c (426 lines)
├── Printing (29-55)         [requires libc]
├── To-string (59-92)        [requires libc]
├── String ops (98-216)      [partly libc: strlen/memcpy; partly pure]
├── Memory (144-163)         [requires libc]
├── Vec (221-322)            [uses ruxen_alloc → malloc]
├── &str (326-372)           [uses libc via strlen]
├── Option/Result (377-405)  [pure — no libc]
├── Fallbacks (410-419)      [pure]
└── Panic (423-426)          [requires libc fprintf]
```

Split into:

**`runtime_core.c`** (always linked):

- Option/Result inspection helpers (line 377-405).
- `ruxen_noop_*` fallbacks (line 410-419) — to be removed with tier-1 B4.
- `ruxen_panic` → exposes a weak symbol that the user's `panic_handler`-marked function overrides. Default weak extension for hosted builds calls `fprintf(stderr, …) + abort()` from `runtime_std.c`.

**`runtime_alloc.c`** (linked when an allocator is available):

- `ruxen_alloc`, `ruxen_dealloc`, `ruxen_realloc` — thin wrappers over a `__ruxen_global_allocator` vtable.
- `Array` primitives (221-322) — built atop `ruxen_alloc`.
- `String` primitives (98-216) — built atop `ruxen_alloc`.

**`runtime_std.c`** (linked when `no-std = false`):

- `fprintf(stderr, …)`-based panic extension.
- Printing (29-55), to-string (59-92), libc-backed `String` ops.
- The default global allocator (wraps `malloc`/`free`).

### 5.2 Directive lifecycle

```
Source:  def my_panic(info: &PanicInfo) -> !
           panic_handler
           ...
         end
         │
         ▼
Parser: Directive { name: "panic_handler", args: [] }
         │
         ▼
Resolver: validates exactly one panic_handler directive in the crate; stores DefId in SymbolTable
         │
         ▼
Typeck:  validates signature (&PanicInfo) -> Never
         │
         ▼
MIR:     adds a MirFunction alias: symbol "ruxen_panic" → user's function (strong)
         │
         ▼
Codegen: emits the user function with LLVM/Cranelift external linkage + attributes
```

Analogous pipeline for `global_allocator` and `no_mangle` directives.

### 5.3 Entry point handling

A hosted binary gets a synthesized `main` (already the case). A no_std binary gets *no* main. Instead:

- The user's public `def main` becomes a function with LLVM linkage `External`, no wrapping, no argc/argv synthesis.
- The user is responsible for providing their own `_start` (or whatever the target needs).

For embedded ARM, users typically write:

```ruxen
let VECTORS: Array[USize] = array![ ..., main as USize, ... ]
no_mangle :VECTORS
link_section :VECTORS, ".vector_table"
```

Linker scripts handle the rest. Out of scope to provide a linker script; we document the pattern.

### 5.4 Drop elaboration in no-std

Tier-1 B1 fix is non-negotiable for no_std. Without real Drop, any program that uses `Array` leaks on every allocation. The fix is in tier-1 scope (phase 1 per tier1_00.md).

After B1 lands:

- `Array.drop` calls `ruxen_alloc.dealloc(ptr, layout)`.
- `String.drop` likewise.
- User-defined classes that include `Drop` likewise.

No-std-specific extra: without an allocator, `Array` / `String` are unavailable — users see compile errors when attempting to construct them. This is enforced by making those types live in `core.alloc.*` which is only importable when a `global_allocator` directive is present.

### 5.5 Compile-time validation

The resolver walks directives in a second pass:

1. Count `panic_handler` directive occurrences. `!= 1` in a no_std binary → error "no_std binary requires exactly one panic_handler; found {N}".
2. Count `global_allocator` directive occurrences. `> 1` → error. `0` + `core.alloc.*` usage → warn, link-time error expected.
3. Reject `panic_handler` / `global_allocator` directives in hosted builds with a note "these directives have effect only in no-std builds."

### 5.6 `core` vs `std` split via cfg

Stdlib source files are partitioned:

```
share/ruxen/std/
├── core/
│   ├── prelude.rx
│   ├── option.rx
│   ├── result.rx
│   ├── mem.rx
│   ├── ptr.rx
│   ├── slice.rx
│   └── alloc/
│       ├── mod.rx
│       ├── array.rx
│       ├── string.rx
│       └── box.rx
└── std/
    ├── prelude.rx
    ├── io.rx
    ├── fs.rx
    ├── env.rx
    ├── process.rx
    ├── net.rx
    ├── time.rx
    ├── path.rx
    └── hash/
        └── default_hasher.rx
```

Resolver behavior:

- Always register `core.*` items.
- If `no-std = true`, do *not* register `std.*` items. Resolve `use std.io` to an error: "`std.io` is not available in no-std builds".
- `std.prelude` re-exports from `core.prelude` plus hosted items.
- `core.prelude` is the auto-imported set for no-std.

### 5.7 panic! macro expansion

Today (tier-1 §7.4 plan):

```
panic!("bad input: {}", x)
  expands to:
  {
    let __msg = format!("bad input: {}", x)
    ruxen_panic_with_location(&__msg, file!(), line!(), col!())
  }
```

For no-std, there's no `format!` (requires `String` + allocator). Alternatives:

- **a)** Require a `global_allocator` directive for any use of `panic!` with format args. Plain `panic!("string-literal")` works without an allocator; `panic!("{}", x)` requires one.
- **b)** Ship a no-alloc formatter (write into a fixed-size stack buffer). Matches Rust's `core.fmt.write` + `core.fmt.Arguments`.

Recommend **(b)** for v1 — simpler users' stories, at the cost of a 200-byte stack buffer per panic. Buffer overrun is silently truncated; defensive, not perfect.

### 5.8 Linker invocation changes

`codegen/object.rs:52-92` grows a branch:

```rust
let is_no_std = opts.no_std;
let mut cmd = Command::new(linker);

if is_no_std {
    cmd.arg("-nostdlib");
    // Don't add -lc -lm
} else {
    cmd.arg("-lc").arg("-lm");
}
cmd.arg(&obj_path).arg(runtime_o).arg("-o").arg(output_path);

for flag in &opts.link_args { cmd.arg(flag); }
```

For the wasm32-unknown-unknown case (doc 03), `-nostdlib` is redundant with `wasm-ld`'s defaults but doesn't hurt.

## 6. Implementation Plan — files to touch

### New files

- `crates/ruxen-core/runtime/runtime_core.c` — the always-linked subset.
- `crates/ruxen-core/runtime/runtime_alloc.c` — allocator-based subset.
- `crates/ruxen-core/runtime/runtime_std.c` — hosted-only subset.
- `crates/ruxen-core/runtime/runtime_common.h` — shared typedefs.
- `share/ruxen/std/core/prelude.rx`, `core/option.rx`, `core/result.rx`, `core/mem.rx`, `core/ptr.rx`.
- `share/ruxen/std/core/alloc/mod.rx`, `alloc/vec.rx`, `alloc/string.rx`, `alloc/box.rx`.
- `share/ruxen/std/core/panic.rx` — `PanicInfo` definition.

### Touched files

- `crates/ruxen-core/runtime/runtime.c` — gutted; becomes `runtime_std.c` minus the common bits.
- `crates/ruxen-core/src/parser/mod.rs:1572-1610` — directive parser accepts `panic_handler`, `global_allocator`, `no_mangle`, `no_std`, `cfg`.
- `crates/ruxen-core/src/parser/ast.rs` — directive variant tags.
- `crates/ruxen-core/src/resolve/mod.rs:97-343` — skip std-registered items when `no-std = true`; split `register_builtins` into `register_core_builtins` and `register_std_builtins`.
- `crates/ruxen-core/src/hir/nodes.rs` — `HirFunction` + `HirItem` gain `is_panic_handler`, `is_no_mangle`.
- `crates/ruxen-core/src/mir/nodes.rs` — `MirProgram` gains `panic_handler: Option<String>`, `global_allocator: Option<String>`, `no_std: bool`.
- `crates/ruxen-core/src/codegen/llvm/emit.rs` / `cranelift.rs` — emit panic handler as strong `ruxen_panic`; emit global allocator functions.
- `crates/ruxen-core/src/codegen/object.rs:52-92` — conditional linker flags (no_std → `-nostdlib`, drop `-lc -lm`).
- `crates/ruxen-core/src/codegen/mod.rs` — `find_runtime_core()`, `find_runtime_std()`, `find_runtime_alloc()`.
- `crates/ruxen-cli/src/manifest.rs:7-47` — `[package] no-std: bool`.
- `crates/ruxen-cli/src/manifest.rs:100-118` — `[profile.*] panic: String` (accept `"abort"` / `"unwind"`; error on unwind for v1).
- `crates/ruxen-cli/src/build.rs` — thread `no_std` through `compile_project`.

### Tests

- `crates/ruxen-core/tests/no_std_basic.rs` — compiles a no_std program with a user-provided panic handler and asserts the resulting ELF has *no* libc imports (verified via `ldd` / `nm`).
- `crates/ruxen-core/tests/no_std_panic_handler.rs` — missing handler → compile error with specific message.
- `crates/ruxen-core/tests/no_std_global_allocator.rs` — `Array` usage without allocator → link error (caught and reported by `ruxen build`).
- `crates/ruxen-core/tests/no_std_drop.rs` — `Array` gets dropped correctly (tier-1 B1 regression).
- Integration test: an example `examples/06-embedded-qemu/` (if we add it) that builds for `thumbv7em-none-eabihf` and boots in QEMU.

## 7. Interactions with Other Tiers

- **Tier 1 stdlib.** §6.3 of tier1_01_stdlib.md defers the `core` vs `std` split; this doc cashes that check. The stdlib source layout (§5.6 above) is the concrete proposal.
- **Tier 1 drop (B1).** Hard prerequisite. No_std with leaking allocations is unusable.
- **Tier 1 derive (B2).** `layout c` and structural-mixin includes are untangled before we add `panic_handler` / `global_allocator` / `no_mangle` directives — otherwise they all stuff strings into the same `derive_traits: Vec<String>` field.
- **Tier 4.02 cross-compilation.** Embedded targets (`thumbv7em-none-eabihf`, `riscv32imac-unknown-none-elf`) imply no-std. The cross-compile plumbing must already accept a triple whose OS is `none`.
- **Tier 4.03 WASM.** `wasm32-unknown-unknown` with `no-std = true` is the ideal WASM mode — drops the bundled dlmalloc, user brings their own allocator. Doc 03's runtime_wasm.c becomes a special case of no_std with a default-shipped allocator.
- **Tier 4.05 stable ABI.** `no_mangle` directive lives here; cbindgen (doc 05) consumes it.
- **Tier 4.06 CI.** A no_std smoke-test matrix entry (build a trivial no_std binary, dump its symbols, grep for the absence of `malloc`) gates regressions.

## 8. Phasing

### Phase 4a — Directive plumbing (1 week, after tier-1 B2)

1. Extend directive parser to accept `no_std`, `panic_handler`, `global_allocator`, `no_mangle`, `cfg`.
2. AST + HIR + MIR fields.
3. Resolver validation (count, signature).
4. **Exit:** a program with `def foo(info: &PanicInfo) -> ! ... end` carrying an in-body `panic_handler` directive parses, type-checks, and the resolver reports the panic handler's DefId.

### Phase 4b — Runtime split (1 week)

1. Split `runtime.c` into `runtime_core.c` / `runtime_std.c` (no `runtime_alloc.c` yet — just `runtime_core` + `runtime_std`).
2. `find_runtime_obj` learns to link both when hosted, only core when no-std.
3. `ruxen_panic` becomes a weak symbol in `runtime_core.c`, strong in `runtime_std.c`.
4. **Exit:** a hosted build's binary is bit-compatible with today's (regression-tested by running every current test).

### Phase 4c — no_std linker path (1 week)

1. `[package] no-std = true` parses.
2. `-nostdlib`, drop `-lc -lm` when no_std.
3. User's `panic_handler`-marked function replaces the weak `ruxen_panic` at link time.
4. `no_mangle` directive emits the function with its Ruxen name (no mangling).
5. **Exit:** a no_std "loop forever" program builds, and `nm` on the output shows zero libc imports.

### Phase 4d — core vs std split (2 weeks)

1. Bisect stdlib source under `share/ruxen/std/core/*` and `share/ruxen/std/std/*`.
2. Resolver skips `std.*` under no_std.
3. `core.alloc.{Array, String, Box, Map, Set}` exist and depend on a `global_allocator` directive.
4. `Allocator` mixin + `Layout`.
5. `__ruxen_global_allocator` vtable dispatch in `runtime_alloc.c`.
6. `panic!` macro: switch to no-alloc formatter for core, full `format!` for std.
7. **Exit:** a no_std binary that uses `core.alloc.Array` with a user-supplied `global_allocator` directive builds, runs, and drops correctly (no leaks).

### Phase 4e — Embedded target sample (0.5 week, if prioritized)

1. `examples/06-embedded-qemu/` — Cortex-M hello-world with a linker script + qemu-system-arm harness.
2. CI smoke test: build for `thumbv7em-none-eabihf`, boot in qemu, assert a single UART write.

### Phase 4f — panic = "unwind" (post-v1)

Landing-pad emission, DWARF CFI, libunwind integration. Months of work. Out of v1.

## 9. Open Questions & Risks

1. **Default panic strategy.** `abort` is the safe default. But users who've seen Rust's `panic = "unwind"` may expect unwinding. Recommend: document explicitly in the book that v1 is abort-only; `unwind` is v2.
2. **Two allocators.** Can a crate have one allocator for `Array` and another for `Box`? Rust says no (one `#[global_allocator]` per binary). Recommend: same rule. Multi-allocator is advanced and rare.
3. **`format!` in no_std.** Proposal §5.7 picks (b) — no-alloc formatter with a fixed buffer. Size? 256 bytes. Truncation is documented.
4. **`println!` in no_std.** It doesn't exist. Users write to an MMIO register or call a host function. Is `panic!("...")` the only reporting mechanism? Probably yes, for stdin-less targets.
5. **ABI stability for `PanicInfo`.** If we later add fields, old no_std user code breaks. Recommend: `PanicInfo` is `#[non_exhaustive]` (Ruxen analog: in-body `sealed` directive). v1 exposes only `message`, `file`, `line`, `col`.
6. **Drop in no_std without an allocator.** If a user has `class Foo` with a destructor that calls `println!`, their no_std build fails at link time because `println!` pulls `runtime_std.c` which pulls libc. Recommend: document clearly; add `cfg(not(no_std))`-style gating so users can write conditional drops.
7. **`Result.map` / `Option.unwrap`** that panic in no_std: these ultimately call `ruxen_panic` — which is satisfied by the user's handler. Fine.
8. **`cfg(no_std)` vs `cfg(not(feature = "std"))`.** Rust uses the feature idiom. Ruxen should too? Recommend: `no-std` manifest key *implies* a `core` feature and negates a `std` feature. Users write `cfg(feature = "std")` to gate hosted-only code paths. Cleaner than a bespoke `no_std` cfg predicate.
9. **Link-line `--gc-sections`.** Embedded users demand it to strip unused symbols. Recommend: don't emit it by default; document in the book that `[target.<triple>].link-args = ["--gc-sections"]` is recommended for binary-size targets.
10. **Shipping `runtime_core.o` per target.** Same problem as doc 02 §5.6: precompiled or source? Recommend: source. `runtime_core.c` is ~100 LOC and compiles in milliseconds with the target's toolchain. For embedded, the user likely has the cross compiler already.
11. **`no_mangle` collisions.** Two `no_mangle`-marked functions with the same Ruxen name in different modules collide at link time. Rust has this problem too; error clearly at resolver time if we can see both.
12. **Static initialization ordering.** A `global_allocator`-marked `let A = Foo.new` — when is `Foo.new` called? For a simple `struct { ... }` with no runtime init, this is a constant. For anything with a constructor, we'd need a static-init path. Recommend v1: `global_allocator`-marked values must be constructible via a `const` expression. Error otherwise.
13. **Missing `core.alloc.Array` in a no_std build** with no global allocator: link error. Can we make this a compile error instead? It's resolvable at link time today; resolver-time errors require cross-item analysis. Recommend: post-link error parsing in `ruxen build` that maps `undefined symbol: __ruxen_global_allocator` to a friendlier message.
14. **Interaction with tier-1 B4 (noop fallback).** `ruxen_noop_passthrough` et al. live in `runtime.c` today. In no_std they must live in `runtime_core.c`. When tier-1 B4 removes them, no_std inherits the cleanup.

## 10. Acceptance Criteria

> **Pass status (2026-06-12).** The non-slip bar — a no_std host binary
> builds+runs with E1400 enforced — is met (see the items marked below). The
> `no_std`/`panic_handler`/`global_allocator`/`no_mangle` **source directives**,
> the runtime `.c` split, and the `core`/`std`/`alloc` re-export surface are the
> **staged remainder** (ADR `phase4-no-std-wasm`). They are marked FILED.

Phase 4a — directives parse:

- [ ] **FILED** — the in-body directive parser (`no_std`/`panic_handler`/`global_allocator`/`no_mangle`) is the staged remainder. v1 uses a `--no-std` CLI flag instead of the source directive.

Phase 4b — runtime split:

- [ ] **FILED / OBSOLETE PREMISE** — there is no single `runtime.c` to split: the stdlib runtime is already per-package (`library/std/<pkg>/runtime/*.c`), and a no_std build links NONE of them. A `runtime_core.c` carve is unnecessary for the v1 bar (a pure-scalar no_std unit needs no runtime).

Phase 4c — no_std linker:

- [x] A no_std `--no-std` build links WITHOUT the Ruxen stdlib runtime / `[system_libs]`, and a pure-arithmetic main RUNS (`examples/06-no-std/exit42.rx` → exit 42; `scripts/no_std_verify.sh`). **DELTA:** `--no-std` CLI flag, not `[package] no-std = true` (manifest key is filed). Built on macOS, not Linux.
- [x] Output has no Ruxen stdlib symbols: `nm` shows zero `ruxen_*` (asserted by `no_std_verify.sh`). **DELTA:** on macOS, `libSystem` is still linked (OS mandate) — the strict zero-libc-imports binary is a Linux/embedded target (`_start` shim + raw syscall), FILED.
- [ ] `-nostdlib` in the linker invocation — **FILED** (the macOS bar cannot use `-nostdlib`; the Linux strict variant will).
- [ ] `no_mangle` directive — **FILED** (directive parser is staged remainder).

Phase 4d — core vs std split:

- [x] **E1400 enforced:** heap allocation (`String`/`Array`/`Map`/`Set` construction) in a no_std unit is rejected with a clean, located diagnostic (`compiler/ruxen_core/src/no_std.rs`, `tests/no_std_e1400.rs`, `docs/errors/E1400.md`). This is the spec's "`use core.alloc.Array` without an allocator errors" criterion, enforced at COMPILE time (§9 Q13's preferred outcome) rather than link time.
- [ ] `use std.io` errors at resolve time — **FILED** (needs the core/std resolver split).
- [ ] `global_allocator` + heap types build+run — **FILED** (the `alloc` tier + allocator dispatch).
- [ ] `panic!` in no_std — **FILED** (panic strategy is abort-only per Open Decision #5; the no-alloc formatter is staged).

Phase 4e — embedded target (if prioritized):

- [ ] `examples/06-embedded-qemu/` boots in qemu-system-arm, writes a single message to the UART peripheral, and halts.
- [ ] CI builds the embedded example and runs qemu in `-nographic` mode, greps the UART output for the expected string.
