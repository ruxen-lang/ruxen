# Tier 4.09 — WASM heap + host imports (browser-GUI foundations)

Status: **heap + host imports landed** (2026-06-14) — both wasm foundations work
end-to-end on `wasm32-unknown-unknown`: a heap-allocated `Array` runs in Node
(`examples/07-wasm-heap`), and a module can call host (JS) functions
(`examples/08-wasm-import`). `scripts/wasm_verify.sh` proves all three bars
(math/heap/import). Host imports use a top-level `lib "<module>"` block whose
functions get `wasm-import-module`/`wasm-import-name` attrs (lib name = import
module, `as` symbol = field) — NOT a separate `wasm_import` keyword; the existing
FFI `lib` surface covers it. Next: full String-method surface on wasm, free-list
allocator, then the canvas web backend. See §11.
Extends: [`tier4_03_wasm_target.md`](tier4_03_wasm_target.md) (which landed pure-computation
`wasm32-unknown-unknown`: LLVM backend, `wasm-ld`, automatic exports, no runtime).

## 1. Summary

Tier 4.03 made Ruxen emit a `wasm32-unknown-unknown` *reactor module* for **pure
computation** — no heap (`String`/`Array` unavailable) and no way to call the host.
This tier adds the two foundations a real GUI app needs in the browser:

1. **Heap** — compile a minimal *core subset* of the stdlib C runtime to wasm,
   linked with a **bundled allocator**, so `String` and `Array` work.
2. **Host imports** — a `wasm_import "module", "name"` directive so Ruxen can
   declare functions the host (browser/JS) supplies.

These are the compiler/runtime prerequisites for the larger goal: a Flutter-like
"write once, run everywhere" GUI stack where `quiver` (L2, 100% safe, backend-
agnostic) runs unchanged in the browser on a future `canvas` web backend. This
tier delivers **only the compiler + runtime foundations** — not the web backend,
JS glue, or packaging (later sub-projects).

## 2. North-star context

The framework already has `canvas` (L1 FFI engine) + `quiver` (L2 widgets). Desktop
is verified (macOS/Linux) and Windows compiles. The missing leg is **web**, and its
hard prerequisite is heap+imports on wasm — without heap, no real Ruxen program
runs; without imports, wasm can't drive a browser canvas. Decision (2026-06-13):
build **`wasm32-unknown-unknown` + a bundled allocator** (not wasm32-wasi) because
browsers have no native WASI and a GUI's host interaction is naturally all-explicit
imports — exactly how CanvasKit-style browser wasm works.

## 3. Goals / Non-goals

**Goals**
- `ruxen build --target wasm32-unknown-unknown` produces a `.wasm` that can create
  and use `String` and `Array` (allocate, index, length, push, concat, drop).
- A `wasm_import "mod", "name"` directive that emits LLVM
  `wasm-import-module` / `wasm-import-name` attributes; `wasm-ld` turns these into
  module imports the host satisfies.
- The core runtime subset compiles to wasm via `clang --target=wasm32-unknown-unknown`
  (clang from the same LLVM 18 the wasm backend already requires) and links via the
  existing `wasm-ld` discovery.
- End-to-end pins: a String/Array program runs in Node; an import round-trips.

**Non-goals (explicit)**
- Threads, net, fs, process, async, regex/PCRE2, wall-clock time — these are
  feature-gated **off** for wasm (host-only). They neither make sense nor link on
  browser wasm.
- `wasm32-wasi`, WASI shims.
- The `canvas` web backend, JS loader/event-pump/rAF glue, web packaging.
- The full tier-1 `slot_t` rework. wasm is always little-endian, so the existing
  `(int64_t)ptr` ↔ `(char*)slot` pattern works; we only do the **localized**
  wasm-correctness gating (the 64-bit `_Static_assert`s).

## 4. The libc surface (measured)

The core-heap runtime (`core/runtime/alloc.c`, `array/runtime/vec.c`,
`string/runtime/string.c`, `fmt/runtime/fmt.c`, `hash/runtime/hash.c`) references a
**small, bounded** set of libc symbols:

| Category | Symbols (call counts) | Provided by |
|----------|----------------------|-------------|
| Memory | `malloc`(48) `free`(56) `realloc`(5) `calloc`(4) | vendored **dlmalloc** |
| mem/str | `memcpy`(38) `memset`(3) `strlen`(42) `strcmp`(5) `strncmp`(2) `strchr`(1) `strstr`(7) | tiny freestanding `wasm_libc.c` |
| sort | `qsort`(2) | small vendored impl |
| format | `snprintf`(5) `strtod`(1) | vendored single-header `snprintf` (+ minimal `strtod`) |
| abort/IO | `exit`(10) `fprintf`(7) | `panic.c`: `exit`→`__builtin_trap()`, `fprintf(stderr,…)`→drop or host import |

So the wasm runtime shim is ~one 200–300 line `wasm_libc.c` + vendored dlmalloc +
a vendored `snprintf` header. No multi-month mini-libc.

## 5. Architecture

Three seams change:

```
compiler (ruxen_core)
  parser/HIR/MIR : new `wasm_import "module","name"` directive on FFI fns
  codegen/llvm/emit : emit wasm-import-{module,name} attrs on imported fns
  codegen/mod.rs : wasm path stops "skipping the runtime" — compiles+links the
                   core runtime subset + allocator/libc shim
  codegen/object.rs : `compile_runtime_for_wasm` (clang --target=wasm32…),
                      `emit_wasm_module` takes N objects (user + runtime + shim)

runtime (new wasm build tree)  library/runtime/wasm/
  wasm_libc.c   memcpy/memset/strlen/strcmp/strncmp/strchr/strstr/qsort
  dlmalloc.c    vendored allocator (malloc/free/realloc/calloc)
  printf.h      vendored single-header snprintf (+ strtod shim)
  panic.c       exit→trap, fprintf→drop/host-import
  runtime.h     gate the 64-bit `_Static_assert`s behind !defined(__wasm32__)
```

C→wasm uses **clang from LLVM 18** (`-nostdlib --target=wasm32-unknown-unknown
-O2`); the existing `wasm-ld` discovery (`object.rs`) links the user object + the
runtime/allocator/shim objects with `--no-entry --export-dynamic --allow-undefined`.

### 5.1 Which runtime sources compile to wasm

NOT the whole stdlib. A curated `WASM_RUNTIME_CORE` list (alloc, vec, string, fmt,
hash to start) compiled only on demand. Everything else is gated off for wasm at
the resolve/codegen layer (a unit that pulls in `std.net` etc. on a wasm target is
a clear compile error, not a link failure).

## 6. Build / CLI surface

- `ruxen build --target wasm32-unknown-unknown [--release]` → `target/wasm32-unknown-unknown/<profile>/*.wasm` (unchanged path; now with heap).
- `wasm_import "env","log"` inside a `lib` block declares a host fn; calling it
  emits a wasm import.
- `RUXEN_WASM_CLANG` / existing `RUXEN_WASM_LD` env overrides for the C compiler /
  linker discovery (mirror the wasm-ld override already present).

## 7. Testing & verification

- Rust pin `tests/wasm_heap.rs` (cfg `llvm`): compile a `.rx` using `String`/`Array`,
  instantiate in Node via a `run.mjs`, assert results (e.g. `"abc".len == 3`,
  array push/sum). Mirrors `examples/05-wasm` harness.
- Rust pin `tests/wasm_import.rs`: a `.rx` declaring `wasm_import` calls a host fn
  supplied at instantiation; assert the host saw the call / the return round-trips.
- New `examples/07-wasm-heap/` + `examples/08-wasm-import/` with `run.mjs`.
- `scripts/wasm_verify.sh` extended to build+run the new examples.
- Gate everything on the LLVM feature + presence of `clang`/`wasm-ld` (skip-with-
  notice when absent, like the existing wasm pins).

## 8. Sequenced increments (each ends green + committed)

1. **Gate the 64-bit asserts** in `runtime.h` behind `!defined(__wasm32__)`; verify
   the existing tier4_03 math example still builds+runs. (smallest safe step)
2. **Wasm C-compile path**: `compile_runtime_for_wasm` (clang `--target=wasm32`),
   `RUXEN_WASM_CLANG` discovery; unit-test it compiles a trivial `.c` to a wasm `.o`.
3. **Allocator + libc shim**: vendor dlmalloc + write `wasm_libc.c` + `printf.h` +
   `panic.c`; compile them to wasm objects.
4. **Link the core runtime on the wasm path**: change `mod.rs:691` so wasm links
   `WASM_RUNTIME_CORE` + shim + allocator; **first heap pin** (`String.len`) green.
5. **Array pin**: push/get/sum; **String concat** pin.
6. **Feature-gate non-wasm stdlib** on the wasm target with a clear diagnostic.
7. **Host imports**: parser `wasm_import` → HIR → MIR → LLVM import attrs;
   **import round-trip pin** green.
8. **Examples + `wasm_verify.sh` + docs/CHANGELOG**; flip status to landed.

## 10. Implementation findings (2026-06-13) — revises §5/§8

TDD against the real compiler surfaced that the heap blocker is **not** "the
runtime isn't linked" but "**wasm bootstraps no stdlib at all**":

- `src/ruxenc/src/compile.rs:509` — `if target.is_wasm() { Vec::new() }` — the wasm
  cross path passes an **empty** bootstrap, so `Array`, `String`, `sum`, `puts`
  don't resolve (confirmed: `no field sum on type Array[Int]` when compiling a
  heap program to wasm).
- *Why* the full bootstrap can't just be enabled: it pulls in `dispatch runtime`
  mixin classes whose `__rx_classinfo_*` globals **the LLVM backend cannot lower**
  (confirmed: a native `--backend=llvm` build of an Array program fails with
  `LLVM backend cannot lower DataAddr { __rx_classinfo_TimeSleepFuture }`). Since
  wasm *must* use LLVM, the full bootstrap is impossible until that gap is closed.
- **Good news:** `grep "dispatch runtime"` across the stdlib hits **only**
  `future/src/lib.rx`. `core`, `array`, `string`, `option_result`, `fmt`, `hash`
  are clean — so a curated subset can bootstrap on LLVM/wasm.

**Revised heap architecture — three parts (was "link the runtime"):**
1. **Curated wasm bootstrap** — a target-aware bootstrap that loads the transitive
   closure of `{core, array, string, option_result}` **minus** the dispatch-runtime
   / libc-heavy modules (`future`, `async_*`, `executor`, `time`, `net`, `fs`,
   `sync`, `process`, and `io`'s host-IO surface). Replaces the `Vec::new()` at
   `compile.rs:509`. Requires resolving the stdlib dep graph (the BOOTSTRAP_FILES
   order is not a safe prefix — `array`/`string` load late but must not drag in the
   excluded modules; this needs verifying, not assuming).
2. **Link the subset's C runtime + bundled allocator** — the `mod.rs:691` change
   (compile `WASM_RUNTIME_CORE` via `clang --target=wasm32`, extend `emit_wasm_module`).
3. **Confirm LLVM emits the subset's class machinery** — `Array[T]`/`String` are
   classes; LLVM choked only on `dispatch runtime` class_info, so plain class
   class_info/vtables are *expected* to work, but this is the open risk to verify
   first (cheapest next experiment: enable the curated bootstrap and see what LLVM
   does before touching the allocator).

**Revised increment order:**
- ✅ **Incr 1 (done, committed `b897c13`)** — gate 64-bit asserts for wasm32.
- **Incr 2 (next, de-risk):** make `compile.rs` bootstrap the curated subset for
  wasm and attempt a native `--backend=llvm` build of an Array program → find out
  exactly what LLVM can/can't lower for the subset. *Cheapest way to retire the
  biggest unknown.*
- **Incr 3:** wasm C-compile path (`compile_runtime_for_wasm`) + allocator/libc shim.
- **Incr 4:** link the curated runtime on the wasm path → first heap pin green.
- (then Array/String pins, feature-gate diagnostics for excluded modules, host imports.)

## 9. Open questions / staged remainder

- `snprintf`/`strtod` fidelity: vendor full single-header vs. minimal; revisit if a
  fmt pin needs more than the vendored subset covers.
- Proper tier-1 `slot_t`→`intptr_t` correctness pass (cosmetic on LE wasm) — defer.
- Host-import ergonomics (a `std.wasm` module wrapping common imports) — later.
