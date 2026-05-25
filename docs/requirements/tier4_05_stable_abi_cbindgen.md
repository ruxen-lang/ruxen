# Tier 4.05 — Stable ABI / cbindgen

## 1. Summary & Motivation

Ruxen can *consume* C via `lib "name" ... end` blocks (`crates/ruxen-core/src/parser/mod.rs:1636-1723`, documented in `docs/tutorial/14-ffi.md`). What Ruxen cannot do is *produce* a C-consumable header. A user who writes a Ruxen library and wants to expose it to C, Python (`ctypes`), Ruby (`fiddle`), Node (N-API), Go (`cgo`), Swift, Kotlin, or anything else that speaks the C ABI has no way to tell those languages what Ruxen's public C-ABI `def foo(...)` signatures look like.

This document specifies a header-emission subsystem: a `ruxenc --emit=c-header` mode that walks the typed HIR, finds every public C-ABI function and `layout c` struct in the compilation unit, and writes a valid `.h` file. It also specifies the **stability rules** — what can safely appear in a Ruxen public C ABI, what can't, and what the compiler must reject.

This is intentionally small. Big-ABI questions (Swift-style stable ABI for Ruxen-to-Ruxen dynamic linking) are out of scope. We only care about emitting C headers for the items users explicitly opt into.

## 2. Current State

### 2.1 Incoming FFI (`parser/mod.rs:1570-1730`)

Ruxen already parses:

- Body-level `link` directive for linker flags.
- `lib "name" ... end` blocks (named C libraries).
- `def func(param: T) -> RetType` inside.
- Variadic `...` parameter.

These produce `LibDecl` AST nodes. Typechecking and MIR preserve them. Codegen declares the listed functions as `Linkage::Import` (`codegen/cranelift.rs:98-103`).

### 2.2 Outgoing ABI — nothing

There is **no** public C-ABI `def foo` → exported-C-symbol path. Every Ruxen function is internally-linked by default, with the compiler applying its own name mangling (`Array[T]_push` → `ruxen_array_push` or similar, see `codegen/runtime.rs:47-71`). The C ABI marker today is parser-only — it goes nowhere on the MIR/codegen side.

To make a Ruxen function callable from C today, a user would need to:

1. Reverse-engineer Ruxen's name mangling (undocumented).
2. Manually write a C header matching whatever Ruxen happens to emit.
3. Hope the mangling doesn't change.

None of that is acceptable for an ABI promise.

### 2.3 `layout c` (tier-1 B2)

Ruxen parses `layout c` on struct/class declarations (`parser/mod.rs:499-503`), but:

- The directive args are stuffed into `HirStructDef::derive_traits: Vec<String>` along with structural-mixin includes (tier-1 B2).
- No layout machinery consumes the directive. Structs are always laid out via the compiler's own rules (`crates/ruxen-core/src/codegen/layout.rs`).
- So `layout c` is a lie today. A struct declared `layout c` on the Ruxen side has the *same* layout as one without — which is not guaranteed to match C's layout rules.

**Tier 5's cbindgen cannot start until B2 is fixed and `layout c` actually produces C-layout.**

### 2.4 No `--emit=c-header` flag

`ruxenc` (`crates/ruxenc/src/main.rs:40-67`) lists `--emit=tokens|ast|hir|mir` but nothing ABI-related.

### 2.5 Tutorial claims FFI works in both directions

`docs/tutorial/14-ffi.md:1-3` says "Ruxen can call C libraries directly." No mention of the other direction — which is accurate.

## 3. Goals & Non-Goals

### Goals

1. `ruxenc --emit=c-header <file.rx> -o lib.h` generates a valid, self-contained C header for the public C-ABI surface of the input.
2. `layout c` on structs/classes produces a layout that matches the C ABI for the target triple.
3. A public C-ABI `def foo(...)` emits `foo` as an un-mangled external symbol.
4. A `no_mangle` directive (doc 04 §4.2) carries over: a public C-ABI `def foo` with in-body `no_mangle` exposes literally `foo`.
5. Stability rules: reject Ruxen-only types (`Option`, `Result`, closures, references with lifetimes, generics) at C-ABI boundaries.
6. A compile-baked `ruxen_abi_version()` function consumers call to detect mismatches.
7. `Ruxen.toml`-druxen integration: `[package.cbindgen] generate = true` + `output = "include/lib.h"` produces the header as a build step.
8. Round-trip test: a Ruxen library + generated header + a tiny C main that links against it + invokes the exported function.

### Non-Goals

- C++ headers (`extern "C++"`, mangled names, classes).
- Stable Ruxen-to-Ruxen ABI for dynamic linking.
- Async / coroutine cross-ABI.
- Python / Ruby / Node binding generators (downstream; once the `.h` is stable, `ctypes`/N-API/etc. work off of it).
- Versioning the header format itself — just stamp the compiler version.
- Auto-generating getters/setters for Ruxen classes. The C API surface is whatever the user exposes via a public `def` with `abi "c"`.
- Generic-struct monomorphization across the ABI. `Array[T]` cannot be exposed; users wrap with a fixed-type `RuxenIntArray`.

## 4. Surface

### 4.1 CLI

```
ruxenc --emit=c-header file.rx -o file.h
ruxenc --emit=c-header file.rx                    # writes to stdout
ruxenc --emit=c-header file.rx --include-guard=MYLIB_H
ruxenc --emit=c-header file.rx --prefix=mylib_    # prepends prefix to emitted symbols

ruxen build                                         # triggers header gen if [package.cbindgen] set
```

### 4.2 Manifest

```toml
[package]
name = "mylib"
version = "0.1.0"

[build]
type = "library"

[package.cbindgen]
generate = true                       # emit C header during `ruxen build`
output = "include/mylib.h"            # path relative to project root
include-guard = "MYLIB_H"
prefix = "mylib_"                     # optional symbol prefix
style = "c11"                         # or "c99" — default c11
namespace = []                        # (future) for C++ namespacing; ignored in C
```

### 4.3 Source directives

The existing `link` / `layout` directive syntax extends:

```ruxen
struct Point
  layout c                                          # enforces C-compatible layout
  x: Float32
  y: Float32
end

enum Color
  layout c
  Red
  Green
  Blue
end

enum Color2
  layout c
  cbindgen_alias "RuxenColor"                       # override the emitted C typedef name
  R
  G
end

def add(a: Int32, b: Int32) -> Int32
  abi "c"
  no_mangle
  a + b
end

# Opaque: the C side gets a `typedef struct RuxenFoo RuxenFoo;` but not the layout.
struct Handle
  layout opaque
  # private state
end

def handle_new -> *var Handle
  abi "c"
  # ... returns heap-allocated Handle
end

def handle_free(h: *var Handle)
  abi "c"
  # ... frees
end
```

### 4.4 Generated header example

Input `mylib.rx`:

```ruxen
struct Point
  layout c
  x: Float32
  y: Float32
end

enum Status
  layout c
  Ok
  Error
end

def add_points(a: Point, b: Point) -> Point
  abi "c"
  no_mangle
  Point.new(x: a.x + b.x, y: a.y + b.y)
end

def check(x: Int32) -> Status
  abi "c"
  no_mangle
  if x > 0; Status.Ok; else; Status.Error; end
end
```

Emitted `mylib.h`:

```c
/* Generated by ruxenc 0.2.0 -- DO NOT EDIT. */
#ifndef MYLIB_H
#define MYLIB_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
lib "c" {
#endif

/* Ruxen ABI version stamped into the build. */
uint32_t ruxen_abi_version(void);

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

Every Ruxen type that can appear in a public C-ABI signature maps to a single C type:

| Ruxen | C | Notes |
|---|---|---|
| `Int8` | `int8_t` | |
| `Int16` | `int16_t` | |
| `Int32` / `Int` | `int32_t` | Ruxen's default `Int` is 32-bit |
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
| `Char` | `uint32_t` | Ruxen Char is 32-bit Unicode scalar |
| `*T` / `*var T` | `const T *` / `T *` | Raw pointers only |
| `&T` | `const T *` | With a **warning**: reference lifetimes don't cross the C ABI |
| `&var T` | `T *` | Same warning |
| `struct` with `layout c` | `struct Name { ... }` | Layout matches |
| `enum` with `layout c` (no payload) | `typedef enum Name { ... }` | Field-less |
| `struct` with `layout opaque` | `typedef struct Name Name;` | Forward decl only |
| Function pointer `|T, U| -> R` | `R (*name)(T, U)` | |
| Tuple `(T, U)` | **error** | No tuple equivalent in C |
| `String` | **error** | Non-C-compatible |
| `Array[T]` | **error** | Use a `(T *, size_t)` wrapper |
| `Option[T]` | **error** | Use a nullable pointer or error code |
| `Result[T, E]` | **error** | Use an out-parameter + error code |
| Generic `Foo[T]` | **error** | Monomorphize via newtype: `newtype FooInt = Foo[Int]` |
| Closures | **error** | Use a function pointer |

For errors, emit the precise diagnostic: "type `String` cannot appear in public C-ABI signatures. Use `*const uint8_t` and `size_t` for a byte string, or `char *` for a null-terminated C string.".

### 4.6 ABI version stamping

At header generation time, emit:

```c
/* In the header: */
uint32_t ruxen_abi_version(void);
#define RUXEN_EXPECTED_ABI_VERSION 0x00020000u  /* bake compiler version */
```

Compiler emits:

```ruxen
# Auto-generated; always public
def ruxen_abi_version -> UInt32
  abi "c"
  no_mangle
  0x00020000u
end
```

Version is computed as `(major << 16) | (minor << 8) | patch` from `crates/ruxen-cli/src/version.rs`. Consumers that link dynamically can check at runtime:

```c
if (ruxen_abi_version() != RUXEN_EXPECTED_ABI_VERSION) {
    fprintf(stderr, "Ruxen library ABI mismatch\n");
    exit(1);
}
```

## 5. Architecture / Design

### 5.1 Where the emitter lives

`crates/ruxen-core/src/cbindgen/` — new module. Exposes `pub fn emit_header(program: &HirProgram, opts: &CbindgenOpts) -> Result<String, Vec<Diagnostic>>`.

Not a separate crate in v1 (no external surface yet). Could extract later if we want the `cbindgen` binary to work standalone.

### 5.2 Walking HIR

Inputs:

- `HirProgram` after typeck.
- `SymbolTable` for name resolution.

Walk:

1. Collect every `HirItem::Struct` / `HirItem::Class` / `HirItem::Enum` with an in-body `layout c` directive.
2. Collect every `HirItem::Function` with public visibility and `abi "c"` directive (with or without an in-body `no_mangle`).
3. For each, validate signatures against the §4.5 table. Accumulate diagnostics; emit all at once (don't bail on first).
4. Topologically sort types so forward-refs aren't needed (a struct that contains another struct must come after its dependency).
5. Emit header text.

### 5.3 Layout validation (`layout c`)

For structs/classes:

- Field order matches source order.
- Padding per the target triple's C ABI (SysV AMD64, ARM64 AAPCS, or wasm32's no-padding 4-byte-aligned rules).
- Reject fields whose types are not themselves C-compatible.
- Error if `class` has methods using `self` (those don't cross the ABI; suggest free-functions).

For enums:

- No-payload enums → `enum` in C (discriminant picked by the compiler; document as `int`).
- Payload enums → error: "`layout c` enums with payloads are not yet supported. Use a tagged union struct instead."
- This is restrictive but honest; Rust has the same history and eventually added `#[repr(C, u8)]` for payload enums.

### 5.4 Symbol emission

A public C-ABI `def foo` emits:

- LLVM/Cranelift: linkage `External`, symbol name `foo` (no mangling). Respects in-body `no_mangle` redundantly.
- Header: `ReturnType foo(...);` signature.

If the user writes public `def foo` *without* an `abi "c"` directive, it stays Ruxen-internal — even if marked public. Only `abi "c"`-tagged functions cross the boundary.

`--prefix=mylib_` in the CLI prepends to the emitted C symbol *and* the header's declaration. Implementation: walk the MIR to rewrite the function's export name; walk the HIR to emit the prefixed name in the header.

### 5.5 Layout table

`codegen/layout.rs` already has struct-layout machinery. Extend (or add a sibling) to compute C-layout for `layout c` types. The algorithm:

1. For each field, align to `alignof(T)` in C terms, accumulate offset.
2. Struct alignment = max alignment of all fields.
3. Struct size = total padded to alignment.
4. For wasm32, pointer types are 4 bytes, 4-aligned (C on wasm32 follows this).

Reuse `target-lexicon` (doc 02) to key the layout off the current triple.

### 5.6 Verification

After emitting the header, optionally run `gcc -fsyntax-only mylib.h` (or `clang`) as a sanity check. Behind a `--verify-header` flag. Fails the build if the generated header doesn't compile.

### 5.7 Stable ABI rules (documented)

A printed contract users can rely on. Recommend shipping this as `docs/c-abi.md`:

1. **ABI covers C-exposed items only.** Everything reachable only through Ruxen-to-Ruxen calls has no stability guarantee.
2. **Struct layouts:** `layout c` fields are laid out C-style, padding/alignment follows the C ABI for the target triple. Adding or reordering fields is a breaking change.
3. **Enum discriminants:** assigned in source order starting from 0. Reordering variants is a breaking change. Removing variants is a breaking change. Adding variants is a breaking change *unless* consumers handle the `default:` case.
4. **Function signatures:** every public `def foo(...)` with `abi "c"` is stable as long as its signature doesn't change. Adding an argument is a breaking change. Changing a parameter type is a breaking change.
5. **Symbol names:** `no_mangle`-marked functions are stable. Non-no-mangle `abi "c"` functions get a predictable mangled name the header captures — also stable.
6. **ABI version:** `ruxen_abi_version()` returns `(major << 16) | (minor << 8) | patch`. Major version bumps on compiler-side ABI breaks.
7. **No unwinding across the ABI.** `panic = "abort"` is required for crates emitting a C ABI.

## 6. Implementation Plan — files to touch

### New files

- `crates/ruxen-core/src/cbindgen/mod.rs` — main emit entry point.
- `crates/ruxen-core/src/cbindgen/types.rs` — Ruxen-to-C type mapper + error messages.
- `crates/ruxen-core/src/cbindgen/validate.rs` — signature validation.
- `crates/ruxen-core/src/cbindgen/layout.rs` — C-compatible layout (or extend `codegen/layout.rs`).
- `crates/ruxen-core/src/cbindgen/emit.rs` — header-text generation.
- `docs/c-abi.md` — the printed contract.

### Touched files

- `crates/ruxen-core/src/parser/mod.rs:499-503` — untangle `layout c` from `derive_traits` (tier-1 B2 prework).
- `crates/ruxen-core/src/parser/mod.rs:1572-1610` — parse `no_mangle`, `cbindgen_alias "..."`, `layout opaque` directives.
- `crates/ruxen-core/src/hir/nodes.rs` — `HirStructDef` / `HirClassDef` / `HirEnumDef` gain `repr: Option<Repr>` (`C`, `Rust`, `Transparent`, `Opaque`, etc.).
- `crates/ruxen-core/src/hir/nodes.rs` — `HirFunction` gains `is_no_mangle: bool`, `abi: Option<String>`, `c_alias: Option<String>`.
- `crates/ruxen-core/src/codegen/layout.rs` — C-layout variant when `repr == Repr::C`.
- `crates/ruxen-core/src/codegen/cranelift.rs` + `llvm/emit.rs` — respect `is_no_mangle` / `abi == "c"` in linkage and symbol naming.
- `crates/ruxenc/src/main.rs:27-67` — new `--emit=c-header` handling.
- `crates/ruxen-cli/src/manifest.rs` — `CbindgenConfig` struct nested under `[package]`.
- `crates/ruxen-cli/src/build.rs` — call cbindgen step when `[package.cbindgen].generate = true`.

### Tests

- `crates/ruxen-core/tests/cbindgen_basic.rs` — simple struct + function, verify header text.
- `crates/ruxen-core/tests/cbindgen_reject.rs` — a public `def f(s: String)` with `abi "c"` errors with the §4.5 message.
- `crates/ruxen-core/tests/cbindgen_layout.rs` — `struct Point` with `layout c` + `Float32` fields has sizeof 8 on x86_64 and aarch64 (match what the C compiler would produce).
- `crates/ruxen-core/tests/cbindgen_round_trip.rs` — build a tiny Ruxen lib, generate header, compile a C main with `cc` linking against the `.rlib`, run, assert behavior.
- `crates/ruxen-core/tests/cbindgen_opaque.rs` — `layout opaque` emits forward decl only.
- `crates/ruxen-core/tests/cbindgen_abi_version.rs` — `ruxen_abi_version()` is auto-exported and returns the expected constant.

## 7. Interactions with Other Tiers

- **Tier 1 (derive, B2).** Prework. `layout c` and structural-mixin includes must be disentangled.
- **Tier 1 (drop, B1).** If a user exposes a public `def handle_new -> *var Handle` with `abi "c"`, the corresponding `handle_free` must properly drop the Handle's fields. B1's Drop-in-codegen fix unblocks this.
- **Tier 1 (formatting macros).** Irrelevant; cbindgen doesn't touch format output.
- **Tier 4.01 package manager.** `[package.cbindgen]` lives in `Ruxen.toml`. `ruxen publish` should include the generated `.h` in the tarball if `output = "include/…"` is set.
- **Tier 4.02 cross-compilation.** Layout is target-dependent. Header generated for `aarch64-unknown-linux-gnu` is valid on that triple only. Recommend: emit the triple as a comment in the header (`/* Generated for target: aarch64-unknown-linux-gnu */`) and `#error` if someone #includes it on a mismatched target? Probably overkill. v1: document the triple dependency; users handle it.
- **Tier 4.03 WASM.** WASM does not use C headers — it uses WIT or raw imports. Cbindgen is a no-op for `wasm32-*` targets; `ruxen build --target wasm32-* ` with cbindgen enabled emits a warning and skips.
- **Tier 4.04 no_std.** `no_mangle` directive is shared — defined in doc 04 §4.2, consumed here. Good.
- **Tier 4.06 CI.** A matrix entry that runs `ruxenc --emit=c-header tests/fixtures/cabi_lib.rx -o /tmp/h.h && gcc -fsyntax-only /tmp/h.h` catches header-syntax regressions.

## 8. Phasing

### Phase 5a — Directive cleanup + basic emit (1-2 weeks, depends on tier-1 B2)

1. Untangle `layout c` from `derive_traits` — add `Repr` enum field to struct/class/enum HIR nodes.
2. Parse `no_mangle`, `cbindgen_alias "..."`, `layout opaque` directives.
3. `--emit=c-header` CLI flag.
4. Walk HIR, emit the header for a trivial case: primitives, `layout c` structs of primitives, public C-ABI functions of primitives.
5. **Exit:** §4.4 example produces the shown header; tests pass.

### Phase 5b — Layout + symbol emission (1 week)

1. C-compatible layout in `codegen/layout.rs` triggered by `Repr::C`.
2. Symbol emission: `abi "c"` + `no_mangle` → external-linkage, no-mangle LLVM/Cranelift symbols.
3. Round-trip test: Ruxen lib + generated header + C main links and runs.
4. **Exit:** the round-trip test passes on x86_64-linux and aarch64-linux.

### Phase 5c — Stability rules (1 week)

1. Reject `String`, `Array`, `Option`, `Result`, tuples, closures, generics at C-ABI boundaries with specific error messages.
2. Reject payload enums with `layout c`.
3. Accept `layout opaque` → forward declaration.
4. `ruxen_abi_version()` auto-emitted.
5. `--prefix=…` symbol prefixing.
6. **Exit:** stability-rule violations produce diagnostics matching the §4.5 table.

### Phase 5d — Build integration + docs (0.5 week)

1. `[package.cbindgen]` manifest wiring: generate during `ruxen build` if configured.
2. `--verify-header` post-check.
3. `docs/c-abi.md` — the contract.
4. One public example in `examples/` demonstrating a Ruxen library + C consumer.
5. **Exit:** `ruxen build` on a library with `[package.cbindgen].generate = true` produces the header at the configured path; doc is shipped; example is green in CI.

## 9. Open Questions & Risks

1. **Which enum discriminant type?** The standard C choice is `int`. Rust's `repr(C)` enums with no payload are also `int` by default. Recommend: `int`. Users can pin with a `layout u8` / `layout u32` directive — but we don't implement that in v1 (error: "`layout u8` is not supported").
2. **Anonymous tagged enums?** C doesn't have them (payload enums). We reject. Users who need them define a `struct { int tag; union { ... }; }` manually in C and wrap with Ruxen.
3. **`Option<*T>` at the C boundary.** C has nullable pointers. `Option[*T]` semantically maps to `T *` with NULL meaning nil. Worth a special-case? Recommend v1: no, reject `Option`. v2: special-case pointer-backed `Option`.
4. **`#[non_exhaustive]`-style struct/enum stability.** If users add a field later, C consumers break. Rust has `#[non_exhaustive]` to opt out of construction-compat. Recommend v1: all `layout c` items are "exhaustive" (additions are breaking); document, no directive.
5. **Header output deterministic?** Must be — CI diffs won't tolerate reordering. Recommend: emit in source order (not HashMap iteration order). Write tests.
6. **`#pragma pack` equivalents.** Rust has `#[repr(C, packed)]`. The Ruxen equivalent would combine `layout c` + `layout packed`. Recommend v1: reject the combination with "not yet supported". Workaround: manual field arrangement + `layout c`.
7. **Header includes.** We include `<stdint.h>`, `<stddef.h>`, `<stdbool.h>` always. Should we include `<float.h>`? Recommend: only when the header uses `float`/`double` — minor optimization, easy to get right.
8. **Symbol mangling for `abi "c"` WITHOUT `no_mangle`.** Recommend: also emit unmangled. `abi "c"` without `no_mangle` is meaningless — the linkage is C, but the symbol name is Ruxen-mangled? Choose: `abi "c"` **implies** `no_mangle`. `no_mangle` alone on a function without `abi "c"` errors "`no_mangle` requires `abi \"c\"`."
9. **Generics monomorphization.** A generic `def foo[T](x: T)` with `abi "c"` errors: "generic functions cannot have C ABI". Users wrap with a non-generic dispatch. Unchanged.
10. **C++ support.** `extern "C++"` is a C++-only concept. Not planned. Users wrap manually.
11. **Opaque pointer + lifetime.** `def handle_new(alloc: &Allocator) -> *var Handle` with `abi "c"` — the `&Allocator` parameter is a reference with a lifetime. We accept at the surface (reference ≡ non-null pointer in C), but the caller must ensure aliasing rules. Document as a warning in the header comment.
12. **Versioning the header file.** Do we emit `#define RUXEN_MYLIB_VERSION "0.1.0"`? Useful for consumers. Recommend: yes, as `#define <PREFIX>VERSION "0.1.0"` where PREFIX is the `--prefix` config or `RUXEN_<PACKAGE>_`.
13. **Layout vs ABI across architectures.** x86_64 SysV, aarch64 AAPCS, and wasm32 have different struct layouts for the same source. Recommend: the header is *target-specific*. Emit a `/* Generated for: x86_64-unknown-linux-gnu */` comment. If the user needs multi-target support, they regenerate per target.
14. **DLL / dylib symbol visibility.** On Windows, `__declspec(dllexport)`. On macOS, default visibility is public; on Linux, `__attribute__((visibility("default")))` for `-fvisibility=hidden` builds. Recommend v1: emit nothing; trust the platform default. Revisit if needed.
15. **Interaction with `layout transparent`.** A newtype wrapping a single field should have the same C representation as the inner type. Nice to have; recommend v1: reject (`"layout transparent` not yet supported"). Easy v2 add.

## 10. Acceptance Criteria

Phase 5a:

- [ ] `layout c` is parsed separately from structural-mixin includes (tier-1 B2 resolved).
- [ ] `ruxenc --emit=c-header trivial.rx` prints a valid header for `struct Point` with `layout c` + `Float32 x, y` and `def zero -> Point` with `abi "c"` + `no_mangle`.
- [ ] Output is deterministic (byte-identical across runs).

Phase 5b:

- [ ] The emitted header compiles under `gcc -fsyntax-only -std=c11 -Wall -Werror`.
- [ ] `sizeof(Point)` in the generated header equals `ruxen_sizeof_Point()` (emit a diagnostic helper) on x86_64 and aarch64.
- [ ] A C main `#include`ing the header, linking against the Ruxen `.rlib`, calls `add_points({1,2}, {3,4})` and gets `{4, 6}`.
- [ ] A public `def foo` with `abi "c"` emits symbol `foo` (not `ruxen_foo` or anything mangled) — verified with `nm`.

Phase 5c:

- [ ] A public `def f(s: String)` with `abi "c"` errors at `--emit=c-header`-time with the specific diagnostic from §4.5.
- [ ] A public `def f(x: Option[Int32])` with `abi "c"` errors similarly.
- [ ] `enum E` with `layout c` containing `A(Int32), B` errors: payload enums not supported.
- [ ] A `struct Handle` with `layout opaque` emits `typedef struct Handle Handle;` in the header and nothing else.
- [ ] `ruxen_abi_version()` is auto-declared in the header and auto-defined in the `.rlib`; a C consumer linking + calling gets the expected `(major << 16) | (minor << 8) | patch` value.
- [ ] `ruxenc --emit=c-header --prefix=mylib_ file.rx` emits `mylib_add_points` instead of `add_points`, both in the header and as the `.rlib` symbol.

Phase 5d:

- [ ] `Ruxen.toml` with `[package.cbindgen] generate = true, output = "include/lib.h"` + `ruxen build` produces `include/lib.h` and the `.rlib`.
- [ ] `--verify-header` invokes `cc -fsyntax-only` on the output and fails the build if the header doesn't compile.
- [ ] `docs/c-abi.md` exists and lists the contract from §5.7.
- [ ] An `examples/` entry demonstrates the full Ruxen-lib + generated-header + C-main workflow, and the example builds + runs in CI.
