# Tier 4.04 — no_std / Embedded Mode

## 1. Summary & Motivation

Every Riven program today links against a C runtime (`crates/riven-core/runtime/runtime.c`, 426 lines) that pulls in libc (`stdio.h`, `stdlib.h`, `string.h`) and assumes a hosted environment. The final binary always links `-lc -lm` (`crates/riven-core/src/codegen/object.rs:64-70`). There is no way to turn this off. That eliminates Riven from:

- **Bare-metal embedded.** Cortex-M / RISC-V microcontrollers have no libc and 16KB of RAM. You bring your own panic handler, your own allocator (if any), and your own linker script.
- **Kernel development.** No dynamic allocation, no I/O beyond what you expose through MMIO.
- **WASM32-unknown-unknown.** Technically has no libc; doc 03 works around this by shipping `dlmalloc` in a wasm-specific runtime.
- **OS-level components.** Bootloaders, kernel modules, UEFI apps.

This document specifies a `no_std` mode: a compiler switch + manifest key that tells the toolchain "I'm providing the host environment; don't link libc, don't assume `malloc`, don't emit a default `main`, give me hooks for panic and allocation."

This is also prerequisite work for tier 4.03 WASM `wasm32-unknown-unknown`. Doc 03 §4.4 ships `runtime_wasm.c` as a one-off; doing no_std properly unifies that effort.

## 2. Current State

### 2.1 Runtime (`crates/riven-core/runtime/runtime.c`)

All 426 lines assume a hosted C environment. Every function either:

- Includes `<stdio.h>` and uses `fputs` / `fprintf` / `fflush`.
- Includes `<stdlib.h>` and uses `malloc` / `free` / `realloc` / `exit` / `abort`.
- Includes `<string.h>` and uses `strlen` / `memcpy` / `strcmp`.

Panic (line 423-426):

```c
void riven_panic(const char *msg) {
    fprintf(stderr, "panic: %s\n", msg);
    abort();
}
```

Allocation (line 144-163):

```c
void *riven_alloc(size_t size) { return malloc(size); }
void riven_dealloc(void *p) { free(p); }
void *riven_realloc(void *p, size_t sz) { return realloc(p, sz); }
```

### 2.2 Link line (`crates/riven-core/src/codegen/object.rs:64-70`)

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

`tier1_00 §B1` documents that `MirInst::Drop` is silently discarded in both codegen backends. In hosted mode this leaks memory until process exit. In no_std / embedded, a 16KB microcontroller exhausts its heap in a few Vec allocations. **No_std cannot ship without B1 fixed.**

### 2.5 No attribute infrastructure for panic handler / allocator

Current attribute parser (`parser/mod.rs:1572-1610`) dispatches `@[link]` and (aspirationally) `@[repr]` / `@[derive]`. There is no `@[panic_handler]`, no `@[global_allocator]`, no `@[no_mangle]`. Tier-1 B2 discusses the attribute-handling cleanup.

### 2.6 No `#[cfg(...)]` item-gating

Tier-1 B6 reserves `async`/`await`/`spawn`/`actor`/`send`/`receive` keywords that are never consumed. No symmetric reservation for conditional-compilation attributes. Doc 01 §5.2 introduces `@[cfg(feature = …)]`; doc 04 extends that to `@[cfg(target_os = "none")]` (the embedded convention) and `@[cfg(not(feature = "std"))]`.

## 3. Goals & Non-Goals

### Goals

1. `[package] no-std = true` manifest key (or equivalently: `[features] default = []` with a non-activating `std`-gate feature, Rust-style).
2. `@[no_std]` crate-level attribute (analog of Rust's `#![no_std]`) as an alternative to the manifest key.
3. `@[panic_handler]` attribute on a user function: `fn(msg: &PanicInfo) -> !`.
4. `@[global_allocator]` attribute on a user value or class implementing an `Allocator` trait.
5. `panic = "abort" | "unwind"` in `[profile.*]`, defaulting to `abort`. **v1 implements `abort` only.**
6. A split runtime: `runtime_core.c` (no libc, no malloc, no I/O) linked in *every* build, plus `runtime_std.c` (the current `runtime.c` minus core bits) linked only when `no-std = false`.
7. A `core` module set under `std::core::*` (re-exported from `std`) that works in no_std builds. Anything `std`-prefixed that touches libc or malloc becomes unavailable.
8. `@[no_mangle]` attribute to export a Riven function with its source name (no mangling).
9. Linker-line control: `-nostdlib` added when `no-std = true`; `-lc -lm` removed.
10. Compatibility with tier 4.03: `wasm32-unknown-unknown` with `no-std = true` works without `runtime_wasm.c`'s bundled `dlmalloc` — users ship their own.

### Non-Goals

- Unwinding panics in v1. `panic = "unwind"` parses and errors with "not yet supported."
- An opinionated bundled allocator. We provide the `Allocator` trait + `@[global_allocator]` hook; the user brings their own (analogous to Rust's `#[global_allocator] static A: MyAlloc = MyAlloc;`).
- Embedded-specific stdlib (`embedded-hal`-equivalent). That's downstream ecosystem work.
- A ready-made `cortex-m-rt`-equivalent. Out of scope.
- Hot-patching / live reload. Out of scope.
- Multiple allocators per crate. One `@[global_allocator]` per binary, Rust-style.

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

### 4.2 Source attributes

**Crate-level** (top of `src/lib.rvn` or `src/main.rvn`):

```riven
@[no_std]                                                   # alternative to [package] no-std = true

use core::Option
use core::Result
use core::panic::PanicInfo
```

**Panic handler** (required in no_std binaries):

```riven
@[panic_handler]
def my_panic(info: &PanicInfo) -> !
  # ... do whatever makes sense for the target ...
  loop { }
end
```

**Global allocator** (required for no_std if `Vec`/`String`/`Hash`/`Set` are used):

```riven
class MyAllocator
  pub def alloc(&self, size: USize, align: USize) -> *mut UInt8
    # ... MMIO or user-provided allocator ...
  end
  pub def dealloc(&self, ptr: *mut UInt8, size: USize, align: USize)
    # ... ...
  end
end

@[global_allocator]
let ALLOCATOR: MyAllocator = MyAllocator.new
```

**No-mangle** (for exporting a C-callable function):

```riven
@[no_mangle]
pub extern "C" def app_main
  # ... called from assembly or a crt0 you wrote ...
end
```

### 4.3 `core` namespace

The stdlib is bisected. Items available in `core` (no-alloc, no-I/O, no-libc):

- Primitives: `Int*`, `UInt*`, `Float*`, `Bool`, `Char`, `USize`, `ISize`.
- `Option`, `Result`.
- `Range`, `RangeInclusive`.
- Traits: `Copy`, `Clone`, `Drop`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`, `Default`, `From`, `Into`, `TryFrom`, `TryInto`, `Iterator`, `IntoIterator`, `FromIterator`.
- `mem::size_of`, `mem::align_of`, `mem::replace`, `mem::swap`, `mem::take`.
- `ptr::null`, `ptr::null_mut`, `ptr::read`, `ptr::write`, `ptr::read_volatile`, `ptr::write_volatile`.
- `slice::from_raw_parts`, `slice::from_raw_parts_mut`.
- Macros: `panic!`, `assert!`, `assert_eq!`, `todo!`, `unimplemented!`.

Items available only in `std` (hosted):

- `io`, `fs`, `env`, `process`, `net`, `time`, `path`, `hash::DefaultHasher`, `prelude::println!`/`eprintln!`/`format!`/`print!`/`eprint!`.
- `Vec`, `String`, `Hash[K,V]`, `Set[T]`, `Box[T]` — but see §4.4 below.

### 4.4 `alloc` tier

Following Rust, there's a middle layer: `alloc` — items that need an allocator but not the full hosted environment. Shipped in `core::alloc::*`:

- `Vec[T]`, `String`, `Box[T]`, `Hash[K,V]`, `Set[T]`.
- Blanket impls for `Iterator`-from-collection.
- `alloc::Layout`, `alloc::Allocator` trait (the one `@[global_allocator]` implements).

To pull these in, a no_std crate does:

```riven
@[no_std]
use core::alloc::Vec
use core::alloc::Box
```

If no `@[global_allocator]` is provided and the user tries to use `Vec`, the linker emits an unresolved-symbol error naming `__riven_global_allocator`. Sharp but clear.

### 4.5 `PanicInfo`

```riven
pub struct PanicInfo
  # Minimal. No downcasting; message is an owned slice.
  message: &'static str
  file: &'static str
  line: UInt32
  col: UInt32

  pub def message(&self) -> &str { self.message }
  pub def location(&self) -> (&str, UInt32, UInt32) { (self.file, self.line, self.col) }
end
```

Passed into `panic_handler` by value-of-reference. The compiler synthesizes it at every `panic!` call site, threading `file!`/`line!`/`column!` literals.

### 4.6 Allocator trait

```riven
pub trait Allocator
  def alloc(&self, layout: Layout) -> Result[*mut UInt8, AllocError]
  def dealloc(&self, ptr: *mut UInt8, layout: Layout)

  # Optional, with defaults:
  def alloc_zeroed(&self, layout: Layout) -> Result[*mut UInt8, AllocError]
  def realloc(&self, ptr: *mut UInt8, old: Layout, new: Layout) -> Result[*mut UInt8, AllocError]
end

pub struct Layout
  pub size: USize
  pub align: USize

  pub def self.new(size: USize, align: USize) -> Result[Layout, LayoutError]
  pub def self.for[T] -> Layout                            # compile-time size_of + align_of
end

pub struct AllocError
  # Zero-sized marker
end
```

`@[global_allocator]` binds a `static`-lifetime value that implements `Allocator`; `Vec`, `String`, etc. route their allocations through it by dispatching on a compiler-internal `__riven_global_allocator` symbol.

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
├── Vec (221-322)            [uses riven_alloc → malloc]
├── &str (326-372)           [uses libc via strlen]
├── Option/Result (377-405)  [pure — no libc]
├── Fallbacks (410-419)      [pure]
└── Panic (423-426)          [requires libc fprintf]
```

Split into:

**`runtime_core.c`** (always linked):

- Option/Result inspection helpers (line 377-405).
- `riven_noop_*` fallbacks (line 410-419) — to be removed with tier-1 B4.
- `riven_panic` → exposes a weak symbol that the user's `@[panic_handler]` overrides. Default weak impl for hosted builds calls `fprintf(stderr, …) + abort()` from `runtime_std.c`.

**`runtime_alloc.c`** (linked when an allocator is available):

- `riven_alloc`, `riven_dealloc`, `riven_realloc` — thin wrappers over a `__riven_global_allocator` vtable.
- `Vec` primitives (221-322) — built atop `riven_alloc`.
- `String` primitives (98-216) — built atop `riven_alloc`.

**`runtime_std.c`** (linked when `no-std = false`):

- `fprintf(stderr, …)`-based panic impl.
- Printing (29-55), to-string (59-92), libc-backed `String` ops.
- The default global allocator (wraps `malloc`/`free`).

### 5.2 Attribute lifecycle

```
Source:  @[panic_handler] def my_panic(info: &PanicInfo) -> ! ... end
         │
         ▼
Parser: Attribute { name: "panic_handler", args: [] }
         │
         ▼
Resolver: validates exactly one @[panic_handler] in the crate; stores DefId in SymbolTable
         │
         ▼
Typeck:  validates signature (&PanicInfo) -> Never
         │
         ▼
MIR:     adds a MirFunction alias: symbol "riven_panic" → user's function (strong)
         │
         ▼
Codegen: emits the user function with LLVM/Cranelift external linkage + attributes
```

Analogous pipeline for `@[global_allocator]` and `@[no_mangle]`.

### 5.3 Entry point handling

A hosted binary gets a synthesized `main` (already the case). A no_std binary gets *no* main. Instead:

- The user's `pub def main` becomes a function with LLVM linkage `External`, no wrapping, no argc/argv synthesis.
- The user is responsible for providing their own `_start` (or whatever the target needs).

For embedded ARM, users typically write:

```
@[no_mangle] @[link_section(".vector_table")]
pub let VECTORS: [USize; N] = [ ..., main as USize, ... ]
```

Linker scripts handle the rest. Out of scope to provide a linker script; we document the pattern.

### 5.4 Drop elaboration in no-std

Tier-1 B1 fix is non-negotiable for no_std. Without real Drop, any program that uses `Vec` leaks on every allocation. The fix is in tier-1 scope (phase 1 per tier1_00.md).

After B1 lands:

- `Vec.drop()` calls `riven_alloc.dealloc(ptr, layout)`.
- `String.drop()` likewise.
- User-defined classes with derived Drop likewise.

No-std-specific extra: without an allocator, `Vec` / `String` are unavailable — users see compile errors when attempting to construct them. This is enforced by making those types live in `core::alloc::*` which is only importable when `@[global_allocator]` is present.

### 5.5 Compile-time validation

The resolver walks attributes in a second pass:

1. Count `@[panic_handler]` occurrences. `!= 1` in a no_std binary → error "no_std binary requires exactly one @[panic_handler]; found {N}".
2. Count `@[global_allocator]` occurrences. `> 1` → error. `0` + `core::alloc::*` usage → warn, link-time error expected.
3. Reject `@[panic_handler]` / `@[global_allocator]` in hosted builds with a note "these attributes have effect only in no-std builds."

### 5.6 `core` vs `std` split via cfg

Stdlib source files are partitioned:

```
share/riven/std/
├── core/
│   ├── prelude.rvn
│   ├── option.rvn
│   ├── result.rvn
│   ├── mem.rvn
│   ├── ptr.rvn
│   ├── slice.rvn
│   └── alloc/
│       ├── mod.rvn
│       ├── vec.rvn
│       ├── string.rvn
│       └── box.rvn
└── std/
    ├── prelude.rvn
    ├── io.rvn
    ├── fs.rvn
    ├── env.rvn
    ├── process.rvn
    ├── net.rvn
    ├── time.rvn
    ├── path.rvn
    └── hash/
        └── default_hasher.rvn
```

Resolver behavior:

- Always register `core::*` items.
- If `no-std = true`, do *not* register `std::*` items. Resolve `use std::io` to an error: "`std::io` is not available in no-std builds".
- `std::prelude` re-exports from `core::prelude` plus hosted items.
- `core::prelude` is the auto-imported set for no-std.

### 5.7 panic! macro expansion

Today (tier-1 §7.4 plan):

```
panic!("bad input: {}", x)
  expands to:
  {
    let __msg = format!("bad input: {}", x)
    riven_panic_with_location(&__msg, file!(), line!(), col!())
  }
```

For no-std, there's no `format!` (requires `String` + allocator). Alternatives:

- **a)** Require `@[global_allocator]` for any use of `panic!` with format args. Plain `panic!("string-literal")` works without an allocator; `panic!("{}", x)` requires one.
- **b)** Ship a no-alloc formatter (write into a fixed-size stack buffer). Matches Rust's `core::fmt::write` + `core::fmt::Arguments`.

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

- `crates/riven-core/runtime/runtime_core.c` — the always-linked subset.
- `crates/riven-core/runtime/runtime_alloc.c` — allocator-based subset.
- `crates/riven-core/runtime/runtime_std.c` — hosted-only subset.
- `crates/riven-core/runtime/runtime_common.h` — shared typedefs.
- `share/riven/std/core/prelude.rvn`, `core/option.rvn`, `core/result.rvn`, `core/mem.rvn`, `core/ptr.rvn`.
- `share/riven/std/core/alloc/mod.rvn`, `alloc/vec.rvn`, `alloc/string.rvn`, `alloc/box.rvn`.
- `share/riven/std/core/panic.rvn` — `PanicInfo` definition.

### Touched files

- `crates/riven-core/runtime/runtime.c` — gutted; becomes `runtime_std.c` minus the common bits.
- `crates/riven-core/src/parser/mod.rs:1572-1610` — attribute parser accepts `panic_handler`, `global_allocator`, `no_mangle`, `no_std`, `cfg`.
- `crates/riven-core/src/parser/ast.rs` — `Attribute` variant tags.
- `crates/riven-core/src/resolve/mod.rs:97-343` — skip std-registered items when `no-std = true`; split `register_builtins` into `register_core_builtins` and `register_std_builtins`.
- `crates/riven-core/src/hir/nodes.rs` — `HirFunction` + `HirItem` gain `is_panic_handler`, `is_no_mangle`.
- `crates/riven-core/src/mir/nodes.rs` — `MirProgram` gains `panic_handler: Option<String>`, `global_allocator: Option<String>`, `no_std: bool`.
- `crates/riven-core/src/codegen/llvm/emit.rs` / `cranelift.rs` — emit panic handler as strong `riven_panic`; emit global allocator functions.
- `crates/riven-core/src/codegen/object.rs:52-92` — conditional linker flags (no_std → `-nostdlib`, drop `-lc -lm`).
- `crates/riven-core/src/codegen/mod.rs` — `find_runtime_core()`, `find_runtime_std()`, `find_runtime_alloc()`.
- `crates/riven-cli/src/manifest.rs:7-47` — `[package] no-std: bool`.
- `crates/riven-cli/src/manifest.rs:100-118` — `[profile.*] panic: String` (accept `"abort"` / `"unwind"`; error on unwind for v1).
- `crates/riven-cli/src/build.rs` — thread `no_std` through `compile_project`.

### Tests

- `crates/riven-core/tests/no_std_basic.rs` — compiles a no_std program with a user-provided panic handler and asserts the resulting ELF has *no* libc imports (verified via `ldd` / `nm`).
- `crates/riven-core/tests/no_std_panic_handler.rs` — missing handler → compile error with specific message.
- `crates/riven-core/tests/no_std_global_allocator.rs` — Vec usage without allocator → link error (caught and reported by `riven build`).
- `crates/riven-core/tests/no_std_drop.rs` — Vec gets dropped correctly (tier-1 B1 regression).
- Integration test: an example `examples/06-embedded-qemu/` (if we add it) that builds for `thumbv7em-none-eabihf` and boots in QEMU.

## 7. Interactions with Other Tiers

- **Tier 1 stdlib.** §6.3 of tier1_01_stdlib.md defers the `core` vs `std` split; this doc cashes that check. The stdlib source layout (§5.6 above) is the concrete proposal.
- **Tier 1 drop (B1).** Hard prerequisite. No_std with leaking allocations is unusable.
- **Tier 1 derive (B2).** `@[repr(C)]` and `@[derive]` are untangled before we add `@[panic_handler]` / `@[global_allocator]` / `@[no_mangle]` — otherwise they all stuff strings into the same `derive_traits: Vec<String>` field.
- **Tier 4.02 cross-compilation.** Embedded targets (`thumbv7em-none-eabihf`, `riscv32imac-unknown-none-elf`) imply no-std. The cross-compile plumbing must already accept a triple whose OS is `none`.
- **Tier 4.03 WASM.** `wasm32-unknown-unknown` with `no-std = true` is the ideal WASM mode — drops the bundled dlmalloc, user brings their own allocator. Doc 03's runtime_wasm.c becomes a special case of no_std with a default-shipped allocator.
- **Tier 4.05 stable ABI.** `@[no_mangle]` attribute lives here; cbindgen (doc 05) consumes it.
- **Tier 4.06 CI.** A no_std smoke-test matrix entry (build a trivial no_std binary, dump its symbols, grep for the absence of `malloc`) gates regressions.

## 8. Phasing

### Phase 4a — Attribute plumbing (1 week, after tier-1 B2)

1. Extend attribute parser to accept `no_std`, `panic_handler`, `global_allocator`, `no_mangle`, `cfg`.
2. AST + HIR + MIR fields.
3. Resolver validation (count, signature).
4. **Exit:** a program with `@[panic_handler] def foo(info: &PanicInfo) -> ! ... end` parses, type-checks, and the resolver reports the panic handler's DefId.

### Phase 4b — Runtime split (1 week)

1. Split `runtime.c` into `runtime_core.c` / `runtime_std.c` (no `runtime_alloc.c` yet — just `runtime_core` + `runtime_std`).
2. `find_runtime_obj` learns to link both when hosted, only core when no-std.
3. `riven_panic` becomes a weak symbol in `runtime_core.c`, strong in `runtime_std.c`.
4. **Exit:** a hosted build's binary is bit-compatible with today's (regression-tested by running every current test).

### Phase 4c — no_std linker path (1 week)

1. `[package] no-std = true` parses.
2. `-nostdlib`, drop `-lc -lm` when no_std.
3. User's `@[panic_handler]` replaces the weak `riven_panic` at link time.
4. `@[no_mangle]` emits the function with its Riven name (no mangling).
5. **Exit:** a no_std "loop forever" program builds, and `nm` on the output shows zero libc imports.

### Phase 4d — core vs std split (2 weeks)

1. Bisect stdlib source under `share/riven/std/core/*` and `share/riven/std/std/*`.
2. Resolver skips `std::*` under no_std.
3. `core::alloc::{Vec, String, Box, Hash, Set}` exist and depend on `@[global_allocator]`.
4. `Allocator` trait + `Layout`.
5. `__riven_global_allocator` vtable dispatch in `runtime_alloc.c`.
6. `panic!` macro: switch to no-alloc formatter for core, full `format!` for std.
7. **Exit:** a no_std binary that uses `core::alloc::Vec` with a user-supplied `@[global_allocator]` builds, runs, and drops correctly (no leaks).

### Phase 4e — Embedded target sample (0.5 week, if prioritized)

1. `examples/06-embedded-qemu/` — Cortex-M hello-world with a linker script + qemu-system-arm harness.
2. CI smoke test: build for `thumbv7em-none-eabihf`, boot in qemu, assert a single UART write.

### Phase 4f — panic = "unwind" (post-v1)

Landing-pad emission, DWARF CFI, libunwind integration. Months of work. Out of v1.

## 9. Open Questions & Risks

1. **Default panic strategy.** `abort` is the safe default. But users who've seen Rust's `panic = "unwind"` may expect unwinding. Recommend: document explicitly in the book that v1 is abort-only; `unwind` is v2.
2. **Two allocators.** Can a crate have one allocator for `Vec` and another for `Box`? Rust says no (one `#[global_allocator]` per binary). Recommend: same rule. Multi-allocator is advanced and rare.
3. **`format!` in no_std.** Proposal §5.7 picks (b) — no-alloc formatter with a fixed buffer. Size? 256 bytes. Truncation is documented.
4. **`println!` in no_std.** It doesn't exist. Users write to an MMIO register or call a host function. Is `panic!("...")` the only reporting mechanism? Probably yes, for stdin-less targets.
5. **ABI stability for `PanicInfo`.** If we later add fields, old no_std user code breaks. Recommend: `PanicInfo` is `#[non_exhaustive]` (Riven analog: `@[sealed]`). v1 exposes only `message`, `file`, `line`, `col`.
6. **Drop in no_std without an allocator.** If a user has `class Foo` with a destructor that calls `println!`, their no_std build fails at link time because `println!` pulls `runtime_std.c` which pulls libc. Recommend: document clearly; add `@[cfg(not(no_std))]`-style gating so users can write conditional drops.
7. **`Result.map` / `Option.unwrap`** that panic in no_std: these ultimately call `riven_panic` — which is satisfied by the user's handler. Fine.
8. **`@[cfg(no_std)]` vs `@[cfg(not(feature = "std"))]`.** Rust uses the feature idiom. Riven should too? Recommend: `no-std` manifest key *implies* a `core` feature and negates a `std` feature. Users write `@[cfg(feature = "std")]` to gate hosted-only code paths. Cleaner than a bespoke `no_std` cfg predicate.
9. **Link-line `--gc-sections`.** Embedded users demand it to strip unused symbols. Recommend: don't emit it by default; document in the book that `[target.<triple>].link-args = ["--gc-sections"]` is recommended for binary-size targets.
10. **Shipping `runtime_core.o` per target.** Same problem as doc 02 §5.6: precompiled or source? Recommend: source. `runtime_core.c` is ~100 LOC and compiles in milliseconds with the target's toolchain. For embedded, the user likely has the cross compiler already.
11. **`no_mangle` collisions.** Two `@[no_mangle]` functions with the same Riven name in different modules collide at link time. Rust has this problem too; error clearly at resolver time if we can see both.
12. **Static initialization ordering.** `@[global_allocator] let A = Foo.new` — when is `Foo.new` called? For a simple `struct { ... }` with no runtime init, this is a constant. For anything with a constructor, we'd need a static-init path. Recommend v1: `@[global_allocator]` values must be constructible via a `const` expression. Error otherwise.
13. **Missing `core::alloc::Vec` in a no_std build** with no global allocator: link error. Can we make this a compile error instead? It's resolvable at link time today; resolver-time errors require cross-item analysis. Recommend: post-link error parsing in `riven build` that maps `undefined symbol: __riven_global_allocator` to a friendlier message.
14. **Interaction with tier-1 B4 (noop fallback).** `riven_noop_passthrough` et al. live in `runtime.c` today. In no_std they must live in `runtime_core.c`. When tier-1 B4 removes them, no_std inherits the cleanup.

## 10. Acceptance Criteria

Phase 4a — attributes parse:

- [ ] `@[no_std]` parses; emitted in `HirCrate`.
- [ ] `@[panic_handler]` on a function with signature `&PanicInfo -> !` type-checks; any other signature errors.
- [ ] Two `@[panic_handler]` functions in one crate errors: "duplicate panic handler".
- [ ] `@[global_allocator]` on a const value of a type implementing `Allocator` type-checks.
- [ ] `@[no_mangle]` on a function emits the function symbol verbatim (verified via `nm`).

Phase 4b — runtime split:

- [ ] Host build links `runtime_core.o` + `runtime_std.o` (two object files, visible in `cc -v` output).
- [ ] All current tests pass.
- [ ] Binary size change < 1%.

Phase 4c — no_std linker:

- [ ] `[package] no-std = true` + a trivial `loop { }` main + `@[panic_handler]` builds on Linux.
- [ ] Output has no libc imports: `nm --undefined-only output` lists only user-defined / `runtime_core` symbols.
- [ ] `-nostdlib` appears in the linker invocation (verified with `RIVEN_VERBOSE=1 riven build`).
- [ ] `@[no_mangle] pub extern "C" def app_main` appears as `app_main` (not `riven_app_main`) in `nm`.

Phase 4d — core vs std split:

- [ ] `use std::io` in a no_std crate errors at resolve time: "`std::io` is not available in no-std builds".
- [ ] `use core::alloc::Vec` in a no_std crate without `@[global_allocator]` errors at link time with a friendly message (or at resolve time, see §9 Q13).
- [ ] A no_std crate with a simple bump allocator `@[global_allocator]` builds and runs `let v = Vec.new(); v.push(1); v.push(2); assert!(v.len == 2); v.drop()` correctly (tier-1 B1 fixed, no leaks).
- [ ] `panic!("literal")` in no_std links and runs.
- [ ] `panic!("{}", x)` in no_std formats into a 256-byte buffer and calls the handler.

Phase 4e — embedded target (if prioritized):

- [ ] `examples/06-embedded-qemu/` boots in qemu-system-arm, writes a single message to the UART peripheral, and halts.
- [ ] CI builds the embedded example and runs qemu in `-nographic` mode, greps the UART output for the expected string.
