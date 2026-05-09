# Tier 4.05 — Stable ABI / cbindgen

## 1. Summary & Motivation

Riven can *consume* C via `lib Name ... end` blocks and `extern "C" ... end` blocks (`crates/riven-core/src/parser/mod.rs:1636-1723`, documented in `docs/tutorial/14-ffi.md`). What Riven cannot do is *produce* a C-consumable header. A user who writes a Riven library and wants to expose it to C, Python (`ctypes`), Ruby (`fiddle`), Node (N-API), Go (`cgo`), Swift, Kotlin, or anything else that speaks the C ABI has no way to tell those languages what Riven's `pub extern "C" def foo(...)` signatures look like.

This document specifies a header-emission subsystem: a `rivenc --emit=c-header` mode that walks the typed HIR, finds every `pub extern "C"` function and `@[repr(C)]` struct in the compilation unit, and writes a valid `.h` file. It also specifies the **stability rules** — what can safely appear in a Riven public C ABI, what can't, and what the compiler must reject.

This is intentionally small. Big-ABI questions (Swift-style stable ABI for Riven-to-Riven dynamic linking) are out of scope. We only care about emitting C headers for the items users explicitly opt into.

## 2. Current State

### 2.1 Incoming FFI (`parser/mod.rs:1570-1730`)

Riven already parses:

- `@[link("foo")]` attribute for linker flags.
- `lib Foo ... end` blocks (named C libraries).
- `extern "C" ... end` blocks (ABI-tagged but nameless).
- `def func(param: T) -> RetType` inside both.
- Variadic `...` parameter.

These produce `LibDecl` / `ExternBlock` AST nodes. Typechecking and MIR preserve them. Codegen declares the listed functions as `Linkage::Import` (`codegen/cranelift.rs:98-103`).

### 2.2 Outgoing ABI — nothing

There is **no** `pub extern "C" def foo` → exported-C-symbol path. Every Riven function is internally-linked by default, with the compiler applying its own name mangling (`Vec[T]_push` → `riven_vec_push` or similar, see `codegen/runtime.rs:47-71`). The `extern "C"` ABI string today is parser-only — it goes nowhere on the MIR/codegen side.

To make a Riven function callable from C today, a user would need to:

1. Reverse-engineer Riven's name mangling (undocumented).
2. Manually write a C header matching whatever Riven happens to emit.
3. Hope the mangling doesn't change.

None of that is acceptable for an ABI promise.

### 2.3 `@[repr(C)]` (tier-1 B2)

Riven parses `@[repr(C)]` on struct/class declarations (`parser/mod.rs:499-503`), but:

- The attribute args are stuffed into `HirStructDef::derive_traits: Vec<String>` along with derive traits (tier-1 B2).
- No layout machinery consumes the attribute. Structs are always laid out via the compiler's own rules (`crates/riven-core/src/codegen/layout.rs`).
- So `@[repr(C)]` is a lie today. A struct declared `@[repr(C)]` on the Riven side has the *same* layout as one without — which is not guaranteed to match C's layout rules.

**Tier 5's cbindgen cannot start until B2 is fixed and `@[repr(C)]` actually produces C-layout.**

### 2.4 No `--emit=c-header` flag

`rivenc` (`crates/rivenc/src/main.rs:40-67`) lists `--emit=tokens|ast|hir|mir` but nothing ABI-related.

### 2.5 Tutorial claims FFI works in both directions

`docs/tutorial/14-ffi.md:1-3` says "Riven can call C libraries directly." No mention of the other direction — which is accurate.

## 3. Goals & Non-Goals

### Goals

1. `rivenc --emit=c-header <file.rvn> -o lib.h` generates a valid, self-contained C header for the public C-ABI surface of the input.
2. `@[repr(C)]` on structs/classes produces a layout that matches the C ABI for the target triple.
3. `pub extern "C" def foo(...)` emits `foo` as an un-mangled external symbol.
4. A `#[no_mangle]` attribute (doc 04 §4.2) carries over: `@[no_mangle] pub extern "C" def foo` exposes literally `foo`.
5. Stability rules: reject Riven-only types (`Option`, `Result`, closures, references with lifetimes, generics) at C-ABI boundaries.
6. A compile-baked `riven_abi_version()` function consumers call to detect mismatches.
7. `Riven.toml`-driven integration: `[package.cbindgen] generate = true` + `output = "include/lib.h"` produces the header as a build step.
8. Round-trip test: a Riven library + generated header + a tiny C main that links against it + invokes the exported function.

### Non-Goals

- C++ headers (`extern "C++"`, mangled names, classes).
- Stable Riven-to-Riven ABI for dynamic linking.
- Async / coroutine cross-ABI.
- Python / Ruby / Node binding generators (downstream; once the `.h` is stable, `ctypes`/N-API/etc. work off of it).
- Versioning the header format itself — just stamp the compiler version.
- Auto-generating getters/setters for Riven classes. The C API surface is whatever the user exposes via `pub extern "C" def`.
- Generic-struct monomorphization across the ABI. `Vec[T]` cannot be exposed; users wrap with a fixed-type `RivenIntVec`.

## 4. Surface

### 4.1 CLI

```
rivenc --emit=c-header file.rvn -o file.h
rivenc --emit=c-header file.rvn                    # writes to stdout
rivenc --emit=c-header file.rvn --include-guard=MYLIB_H
rivenc --emit=c-header file.rvn --prefix=mylib_    # prepends prefix to emitted symbols

riven build                                         # triggers header gen if [package.cbindgen] set
```

### 4.2 Manifest

```toml
[package]
name = "mylib"
version = "0.1.0"

[build]
type = "library"

[package.cbindgen]
generate = true                       # emit C header during `riven build`
output = "include/mylib.h"            # path relative to project root
include-guard = "MYLIB_H"
prefix = "mylib_"                     # optional symbol prefix
style = "c11"                         # or "c99" — default c11
namespace = []                        # (future) for C++ namespacing; ignored in C
```

### 4.3 Source attributes

The existing `@[link]` / `@[repr]` attribute syntax extends:

```riven
@[repr(C)]                                          # enforces C-compatible layout
struct Point
  x: Float32
  y: Float32
end

@[repr(C)]
enum Color
  Red
  Green
  Blue
end

@[repr(C)]
@[cbindgen_alias("RivenColor")]                     # override the emitted C typedef name
enum Color2
  R
  G
end

@[no_mangle]
pub extern "C" def add(a: Int32, b: Int32) -> Int32
  a + b
end

# Opaque: the C side gets a `typedef struct RivenFoo RivenFoo;` but not the layout.
@[repr(opaque)]
pub struct Handle
  # private state
end

pub extern "C" def handle_new -> *mut Handle
  # ... returns heap-allocated Handle
end

pub extern "C" def handle_free(h: *mut Handle)
  # ... frees
end
```

### 4.4 Generated header example

Input `mylib.rvn`:

```riven
@[repr(C)]
pub struct Point
  x: Float32
  y: Float32
end

@[repr(C)]
pub enum Status
  Ok
  Error
end

@[no_mangle]
pub extern "C" def add_points(a: Point, b: Point) -> Point
  Point { x: a.x + b.x, y: a.y + b.y }
end

@[no_mangle]
pub extern "C" def check(x: Int32) -> Status
  if x > 0 then Status.Ok else Status.Error end
end
```

Emitted `mylib.h`:

```c
/* Generated by rivenc 0.2.0 -- DO NOT EDIT. */
#ifndef MYLIB_H
#define MYLIB_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Riven ABI version stamped into the build. */
uint32_t riven_abi_version(void);

typedef struct Point {
    float x;
    float y;
} Point;

typedef enum Status {
    Status_Ok = 0,
    Status_Error = 1,
} Status;

Point add_points(Point a, Point b);
Status check(int32_t x);

#ifdef __cplusplus
}
#endif

#endif /* MYLIB_H */
```

### 4.5 Type-mapping table

Every Riven type that can appear in a `pub extern "C"` signature maps to a single C type:

| Riven | C | Notes |
|---|---|---|
| `Int8` | `int8_t` | |
| `Int16` | `int16_t` | |
| `Int32` / `Int` | `int32_t` | Riven's default `Int` is 32-bit |
| `Int64` | `int64_t` | |
| `UInt8` | `uint8_t` | |
| `UInt16` | `uint16_t` | |
| `UInt32` / `UInt` | `uint32_t` | |
| `UInt64` | `uint64_t` | |
| `USize` | `size_t` | `<stddef.h>` |
| `ISize` | `ptrdiff_t` | |
| `Float32` / `Float` | `float` | |
| `Float64` | `double` | |
| `Bool` | `bool` | `<stdbool.h>`; C99+ |
| `Char` | `uint32_t` | Riven Char is 32-bit Unicode scalar |
| `*T` / `*mut T` | `const T *` / `T *` | Raw pointers only |
| `&T` | `const T *` | With a **warning**: reference lifetimes don't cross the C ABI |
| `&mut T` | `T *` | Same warning |
| `@[repr(C)] struct { ... }` | `struct Name { ... }` | Layout matches |
| `@[repr(C)] enum` (no payload) | `typedef enum Name { ... }` | Field-less |
| `@[repr(C)] @[repr(opaque)] struct` | `typedef struct Name Name;` | Forward decl only |
| Function pointer `fn(T, U) -> R` | `R (*name)(T, U)` | |
| Tuple `(T, U)` | **error** | No tuple equivalent in C |
| `String` | **error** | Non-C-compatible |
| `Vec[T]` | **error** | Use a `(T *, size_t)` wrapper |
| `Option[T]` | **error** | Use a nullable pointer or error code |
| `Result[T, E]` | **error** | Use an out-parameter + error code |
| Generic `Foo[T]` | **error** | Monomorphize via newtype: `struct FooInt = Foo[Int]` |
| Closures | **error** | Use a function pointer |

For errors, emit the precise diagnostic: "type `String` cannot appear in `pub extern \"C\"` signatures. Use `*const uint8_t` and `size_t` for a byte string, or `char *` for a null-terminated C string.".

### 4.6 ABI version stamping

At header generation time, emit:

```c
/* In the header: */
uint32_t riven_abi_version(void);
#define RIVEN_EXPECTED_ABI_VERSION 0x00020000u  /* bake compiler version */
```

Compiler emits:

```riven
# Auto-generated; always public
pub extern "C" def riven_abi_version -> UInt32
  0x00020000u
end
```

Version is computed as `(major << 16) | (minor << 8) | patch` from `crates/riven-cli/src/version.rs`. Consumers that link dynamically can check at runtime:

```c
if (riven_abi_version() != RIVEN_EXPECTED_ABI_VERSION) {
    fprintf(stderr, "Riven library ABI mismatch\n");
    exit(1);
}
```

## 5. Architecture / Design

### 5.1 Where the emitter lives

`crates/riven-core/src/cbindgen/` — new module. Exposes `pub fn emit_header(program: &HirProgram, opts: &CbindgenOpts) -> Result<String, Vec<Diagnostic>>`.

Not a separate crate in v1 (no external surface yet). Could extract later if we want the `cbindgen` binary to work standalone.

### 5.2 Walking HIR

Inputs:

- `HirProgram` after typeck.
- `SymbolTable` for name resolution.

Walk:

1. Collect every `HirItem::Struct` / `HirItem::Class` / `HirItem::Enum` with a `@[repr(C)]` attribute.
2. Collect every `HirItem::Function` with visibility `Public` and ABI `"C"` (either via `pub extern "C"` or `@[no_mangle] pub ...`).
3. For each, validate signatures against the §4.5 table. Accumulate diagnostics; emit all at once (don't bail on first).
4. Topologically sort types so forward-refs aren't needed (a struct that contains another struct must come after its dependency).
5. Emit header text.

### 5.3 Layout validation (`@[repr(C)]`)

For structs/classes:

- Field order matches source order.
- Padding per the target triple's C ABI (SysV AMD64, ARM64 AAPCS, or wasm32's no-padding 4-byte-aligned rules).
- Reject fields whose types are not themselves C-compatible.
- Error if `class` has methods using `self` (those don't cross the ABI; suggest free-functions).

For enums:

- No-payload enums → `enum` in C (discriminant picked by the compiler; document as `int`).
- Payload enums → error: "`@[repr(C)]` enums with payloads are not yet supported. Use a tagged union struct instead."
- This is restrictive but honest; Rust has the same history and eventually added `@[repr(C, u8)]` for payload enums.

### 5.4 Symbol emission

`pub extern "C" def foo` emits:

- LLVM/Cranelift: linkage `External`, symbol name `foo` (no mangling). Respects `@[no_mangle]` redundantly.
- Header: `ReturnType foo(...);` signature.

If the user writes `pub def foo` *without* `extern "C"`, it stays Riven-internal — even if marked `pub`. Only `pub extern "C"` crosses the boundary.

`--prefix=mylib_` in the CLI prepends to the emitted C symbol *and* the header's declaration. Implementation: walk the MIR to rewrite the function's export name; walk the HIR to emit the prefixed name in the header.

### 5.5 Layout table

`codegen/layout.rs` already has struct-layout machinery. Extend (or add a sibling) to compute C-layout for `@[repr(C)]` types. The algorithm:

1. For each field, align to `alignof(T)` in C terms, accumulate offset.
2. Struct alignment = max alignment of all fields.
3. Struct size = total padded to alignment.
4. For wasm32, pointer types are 4 bytes, 4-aligned (C on wasm32 follows this).

Reuse `target-lexicon` (doc 02) to key the layout off the current triple.

### 5.6 Verification

After emitting the header, optionally run `gcc -fsyntax-only mylib.h` (or `clang`) as a sanity check. Behind a `--verify-header` flag. Fails the build if the generated header doesn't compile.

### 5.7 Stable ABI rules (documented)

A printed contract users can rely on. Recommend shipping this as `docs/c-abi.md`:

1. **ABI covers C-exposed items only.** Everything reachable only through Riven-to-Riven calls has no stability guarantee.
2. **Struct layouts:** `@[repr(C)]` fields are laid out C-style, padding/alignment follows the C ABI for the target triple. Adding or reordering fields is a breaking change.
3. **Enum discriminants:** assigned in source order starting from 0. Reordering variants is a breaking change. Removing variants is a breaking change. Adding variants is a breaking change *unless* consumers handle the `default:` case.
4. **Function signatures:** every `pub extern "C" def foo(...)` is stable as long as its signature doesn't change. Adding an argument is a breaking change. Changing a parameter type is a breaking change.
5. **Symbol names:** `@[no_mangle]` functions are stable. Non-no-mangle `pub extern "C"` functions get a predictable mangled name the header captures — also stable.
6. **ABI version:** `riven_abi_version()` returns `(major << 16) | (minor << 8) | patch`. Major version bumps on compiler-side ABI breaks.
7. **No unwinding across the ABI.** `panic = "abort"` is required for crates emitting a C ABI.

## 6. Implementation Plan — files to touch

### New files

- `crates/riven-core/src/cbindgen/mod.rs` — main emit entry point.
- `crates/riven-core/src/cbindgen/types.rs` — Riven-to-C type mapper + error messages.
- `crates/riven-core/src/cbindgen/validate.rs` — signature validation.
- `crates/riven-core/src/cbindgen/layout.rs` — C-compatible layout (or extend `codegen/layout.rs`).
- `crates/riven-core/src/cbindgen/emit.rs` — header-text generation.
- `docs/c-abi.md` — the printed contract.

### Touched files

- `crates/riven-core/src/parser/mod.rs:499-503` — untangle `@[repr(C)]` from `derive_traits` (tier-1 B2 prework).
- `crates/riven-core/src/parser/mod.rs:1572-1610` — parse `@[no_mangle]`, `@[cbindgen_alias("...")]`, `@[repr(opaque)]`.
- `crates/riven-core/src/hir/nodes.rs` — `HirStructDef` / `HirClassDef` / `HirEnumDef` gain `repr: Option<Repr>` (`C`, `Rust`, `Transparent`, `Opaque`, etc.).
- `crates/riven-core/src/hir/nodes.rs` — `HirFunction` gains `is_no_mangle: bool`, `abi: Option<String>`, `c_alias: Option<String>`.
- `crates/riven-core/src/codegen/layout.rs` — C-layout variant when `repr == Repr::C`.
- `crates/riven-core/src/codegen/cranelift.rs` + `llvm/emit.rs` — respect `is_no_mangle` / `abi == "C"` in linkage and symbol naming.
- `crates/rivenc/src/main.rs:27-67` — new `--emit=c-header` handling.
- `crates/riven-cli/src/manifest.rs` — `CbindgenConfig` struct nested under `[package]`.
- `crates/riven-cli/src/build.rs` — call cbindgen step when `[package.cbindgen].generate = true`.

### Tests

- `crates/riven-core/tests/cbindgen_basic.rs` — simple struct + function, verify header text.
- `crates/riven-core/tests/cbindgen_reject.rs` — `pub extern "C" def f(s: String)` errors with the §4.5 message.
- `crates/riven-core/tests/cbindgen_layout.rs` — `@[repr(C)] struct Point { x: Float32, y: Float32 }` has sizeof 8 on x86_64 and aarch64 (match what the C compiler would produce).
- `crates/riven-core/tests/cbindgen_round_trip.rs` — build a tiny Riven lib, generate header, compile a C main with `cc` linking against the `.rlib`, run, assert behavior.
- `crates/riven-core/tests/cbindgen_opaque.rs` — `@[repr(opaque)]` emits forward decl only.
- `crates/riven-core/tests/cbindgen_abi_version.rs` — `riven_abi_version()` is auto-exported and returns the expected constant.

## 7. Interactions with Other Tiers

- **Tier 1 (derive, B2).** Prework. `@[repr(C)]` and `@[derive]` must be disentangled.
- **Tier 1 (drop, B1).** If a user exposes `pub extern "C" def handle_new -> *mut Handle`, the corresponding `handle_free` must properly drop the Handle's fields. B1's Drop-in-codegen fix unblocks this.
- **Tier 1 (formatting macros).** Irrelevant; cbindgen doesn't touch format output.
- **Tier 4.01 package manager.** `[package.cbindgen]` lives in `Riven.toml`. `riven publish` should include the generated `.h` in the tarball if `output = "include/…"` is set.
- **Tier 4.02 cross-compilation.** Layout is target-dependent. Header generated for `aarch64-unknown-linux-gnu` is valid on that triple only. Recommend: emit the triple as a comment in the header (`/* Generated for target: aarch64-unknown-linux-gnu */`) and `#error` if someone #includes it on a mismatched target? Probably overkill. v1: document the triple dependency; users handle it.
- **Tier 4.03 WASM.** WASM does not use C headers — it uses WIT or raw imports. Cbindgen is a no-op for `wasm32-*` targets; `riven build --target wasm32-* ` with cbindgen enabled emits a warning and skips.
- **Tier 4.04 no_std.** `@[no_mangle]` attribute is shared — defined in doc 04 §4.2, consumed here. Good.
- **Tier 4.06 CI.** A matrix entry that runs `rivenc --emit=c-header tests/fixtures/cabi_lib.rvn -o /tmp/h.h && gcc -fsyntax-only /tmp/h.h` catches header-syntax regressions.

## 8. Phasing

### Phase 5a — Attribute cleanup + basic emit (1-2 weeks, depends on tier-1 B2)

1. Untangle `@[repr(C)]` from `derive_traits` — add `Repr` enum field to struct/class/enum HIR nodes.
2. Parse `@[no_mangle]`, `@[cbindgen_alias("...")]`, `@[repr(opaque)]`.
3. `--emit=c-header` CLI flag.
4. Walk HIR, emit the header for a trivial case: primitives, `@[repr(C)]` structs of primitives, `pub extern "C"` functions of primitives.
5. **Exit:** §4.4 example produces the shown header; tests pass.

### Phase 5b — Layout + symbol emission (1 week)

1. C-compatible layout in `codegen/layout.rs` triggered by `Repr::C`.
2. Symbol emission: `pub extern "C"` + `@[no_mangle]` → external-linkage, no-mangle LLVM/Cranelift symbols.
3. Round-trip test: Riven lib + generated header + C main links and runs.
4. **Exit:** the round-trip test passes on x86_64-linux and aarch64-linux.

### Phase 5c — Stability rules (1 week)

1. Reject `String`, `Vec`, `Option`, `Result`, tuples, closures, generics at C-ABI boundaries with specific error messages.
2. Reject payload enums with `@[repr(C)]`.
3. Accept `@[repr(opaque)]` → forward declaration.
4. `riven_abi_version()` auto-emitted.
5. `--prefix=…` symbol prefixing.
6. **Exit:** stability-rule violations produce diagnostics matching the §4.5 table.

### Phase 5d — Build integration + docs (0.5 week)

1. `[package.cbindgen]` manifest wiring: generate during `riven build` if configured.
2. `--verify-header` post-check.
3. `docs/c-abi.md` — the contract.
4. One public example in `examples/` demonstrating a Riven library + C consumer.
5. **Exit:** `riven build` on a library with `[package.cbindgen].generate = true` produces the header at the configured path; doc is shipped; example is green in CI.

## 9. Open Questions & Risks

1. **Which enum discriminant type?** The standard C choice is `int`. Rust's `repr(C)` enums with no payload are also `int` by default. Recommend: `int`. Users can pin with `@[repr(u8)]` / `@[repr(u32)]` — but we don't implement that in v1 (error: "`@[repr(u8)]` is not supported").
2. **Anonymous tagged enums?** C doesn't have them (payload enums). We reject. Users who need them define a `struct { int tag; union { ... }; }` manually in C and wrap with Riven.
3. **`Option<*T>` at the C boundary.** C has nullable pointers. `Option[*T]` semantically maps to `T *` with NULL meaning None. Worth a special-case? Recommend v1: no, reject `Option`. v2: special-case pointer-backed `Option`.
4. **`#[non_exhaustive]`-style struct/enum stability.** If users add a field later, C consumers break. Rust has `#[non_exhaustive]` to opt out of construction-compat. Recommend v1: all `@[repr(C)]` items are "exhaustive" (additions are breaking); document, no attribute.
5. **Header output deterministic?** Must be — CI diffs won't tolerate reordering. Recommend: emit in source order (not HashMap iteration order). Write tests.
6. **`#pragma pack` equivalents.** Rust has `#[repr(C, packed)]`. Recommend v1: reject `@[repr(packed)]` with "not yet supported". Workaround: manual field arrangement + `@[repr(C)]`.
7. **Header includes.** We include `<stdint.h>`, `<stddef.h>`, `<stdbool.h>` always. Should we include `<float.h>`? Recommend: only when the header uses `float`/`double` — minor optimization, easy to get right.
8. **Symbol mangling for pub extern "C" WITHOUT @[no_mangle].** Recommend: also emit unmangled. `pub extern "C"` without `@[no_mangle]` is meaningless — the linkage is C, but the symbol name is Riven-mangled? Choose: `pub extern "C"` **implies** `@[no_mangle]`. `@[no_mangle]` alone on a non-extern function errors "`@[no_mangle]` requires `extern \"C\"`."
9. **Generics monomorphization.** `pub extern "C" def foo[T](x: T)` errors: "generic functions cannot have C ABI". Users wrap with a non-generic dispatch. Unchanged.
10. **C++ support.** `extern "C++"` is a C++-only concept. Not planned. Users wrap manually.
11. **Opaque pointer + lifetime.** `pub extern "C" def handle_new(alloc: &Allocator) -> *mut Handle` — the `&Allocator` parameter is a reference with a lifetime. We accept at the surface (reference ≡ non-null pointer in C), but the caller must ensure aliasing rules. Document as a warning in the header comment.
12. **Versioning the header file.** Do we emit `#define RIVEN_MYLIB_VERSION "0.1.0"`? Useful for consumers. Recommend: yes, as `#define <PREFIX>VERSION "0.1.0"` where PREFIX is the `--prefix` config or `RIVEN_<PACKAGE>_`.
13. **Layout vs ABI across architectures.** x86_64 SysV, aarch64 AAPCS, and wasm32 have different struct layouts for the same source. Recommend: the header is *target-specific*. Emit a `/* Generated for: x86_64-unknown-linux-gnu */` comment. If the user needs multi-target support, they regenerate per target.
14. **DLL / dylib symbol visibility.** On Windows, `__declspec(dllexport)`. On macOS, default visibility is public; on Linux, `__attribute__((visibility("default")))` for `-fvisibility=hidden` builds. Recommend v1: emit nothing; trust the platform default. Revisit if needed.
15. **Interaction with `@[repr(transparent)]`.** A newtype wrapping a single field should have the same C representation as the inner type. Nice to have; recommend v1: reject (`"@[repr(transparent)]` not yet supported"). Easy v2 add.

## 10. Acceptance Criteria

Phase 5a:

- [ ] `@[repr(C)]` is parsed separately from `@[derive]` (tier-1 B2 resolved).
- [ ] `rivenc --emit=c-header trivial.rvn` prints a valid header for `@[repr(C)] struct Point { x: Float32, y: Float32 } @[no_mangle] pub extern "C" def zero -> Point`.
- [ ] Output is deterministic (byte-identical across runs).

Phase 5b:

- [ ] The emitted header compiles under `gcc -fsyntax-only -std=c11 -Wall -Werror`.
- [ ] `sizeof(Point)` in the generated header equals `riven_sizeof_Point()` (emit a diagnostic helper) on x86_64 and aarch64.
- [ ] A C main `#include`ing the header, linking against the Riven `.rlib`, calls `add_points({1,2}, {3,4})` and gets `{4, 6}`.
- [ ] `pub extern "C" def foo` emits symbol `foo` (not `riven_foo` or anything mangled) — verified with `nm`.

Phase 5c:

- [ ] `pub extern "C" def f(s: String)` errors at `--emit=c-header`-time with the specific diagnostic from §4.5.
- [ ] `pub extern "C" def f(x: Option[Int32])` errors similarly.
- [ ] `@[repr(C)] enum E { A(Int32), B }` errors: payload enums not supported.
- [ ] `@[repr(opaque)] pub struct Handle ...` emits `typedef struct Handle Handle;` in the header and nothing else.
- [ ] `riven_abi_version()` is auto-declared in the header and auto-defined in the `.rlib`; a C consumer linking + calling gets the expected `(major << 16) | (minor << 8) | patch` value.
- [ ] `rivenc --emit=c-header --prefix=mylib_ file.rvn` emits `mylib_add_points` instead of `add_points`, both in the header and as the `.rlib` symbol.

Phase 5d:

- [ ] `Riven.toml` with `[package.cbindgen] generate = true, output = "include/lib.h"` + `riven build` produces `include/lib.h` and the `.rlib`.
- [ ] `--verify-header` invokes `cc -fsyntax-only` on the output and fails the build if the header doesn't compile.
- [ ] `docs/c-abi.md` exists and lists the contract from §5.7.
- [ ] An `examples/` entry demonstrates the full Riven-lib + generated-header + C-main workflow, and the example builds + runs in CI.
