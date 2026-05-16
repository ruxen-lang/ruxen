# Spec — Phase 7 FFI surface

**Source docs:**
[docs/requirements/](../../requirements/) (no dedicated doc yet);
canonical surface-syntax §3.7
([ruby-naming.spec.md](../syntax/ruby-naming.spec.md)).

**Status:** shipped Phase 7 (parser + layout; runtime trampolines).

Riven's FFI surface lets user code declare external C symbols and
opt-in unsafe blocks.  This spec covers what the parser accepts and
what the layout rules guarantee.

---

## B1 — `unsafe { ... }` block parses

```riven
unsafe
  ptr.deref
end
```

The block opens a region in which raw-pointer operations are legal.

## B2 — `nil` literal + raw pointer types parse

`nil` is the polymorphic absence literal; in an `unsafe` block it
resolves to a raw-pointer absence value per §3.10 of the canonical
syntax spec.  Raw pointer types are spelled `*T` (read-only) and `*var T`
(writable).

A pointer-typed `nil` reaching outside an `unsafe` block emits
`E-NIL-RAW-OUTSIDE-UNSAFE`.

## B3 — `lib "name"` declaration parses

```riven
lib "c"
  def malloc(n: USize) -> *var Void
end
```

Declares an external shared library; nested `def` declarations
expose its symbols.  Optional `path:` / `version:` options on `lib`
pin the linker name.  There is no separate `extern` block keyword —
every FFI declaration goes inside a `lib "..." ... end` block.

## B4 — `lib "<linkname>"` block parses (C-ABI)

`lib "<linkname>"` blocks declare C-callable functions; parameter
types follow the same `*T` / `*var T` rules as B2.  `lib "c"` is
the canonical spelling for the C standard library.

## B5 — `Void` return type

FFI defs may return `Void` (mapped to Riven's `Unit`).

## B6 — Multi-parameter FFI

Accepts arbitrarily many parameters, mixing scalars and pointers.

## B7 — Variadic C functions

The parser accepts a trailing `...` inside `lib` block `def`
signatures:

```riven
lib "c"
  def printf(fmt: *UInt8, ...) -> Int32
end
```

Variadic arguments at a call site must be primitive scalars or
pointer values — aggregate types passed through `...` produce a
typeck error.

## B8 — `layout c` struct layout

`layout c` at the top of a struct body guarantees C-compatible
layout (fields in declaration order, native alignment, no
reordering).

## B9 — Implicit structural mixin inclusion on struct / class / enum

Structural mixins (`Debug`, `Clone`, `Eq`, `Hashable`, …) are
implicitly included when the fields support them; an explicit
`include D1, D2` inside the body is the loud form and lowers to
the same AST node.  See [implicit_includes.spec.md](../mixins/implicit_includes.spec.md).

## B10 — In-body `include` clause syntax

An `include Foo, Bar` clause inside a struct/class/enum body parses
the loud form of the implicit-include rule.

## B11 — Layout invariants

| Layout                                  | Guarantee                              |
|-----------------------------------------|----------------------------------------|
| Raw pointer (`*T` / `*var T`)           | 8 bytes (64-bit pointer)               |
| `layout packed` struct                  | No padding between fields              |
| `layout transparent`                    | Same layout as inner field             |
| `layout transparent` w/ multiple fields | rejected at typeck                     |
| `layout c` struct                       | Matches plain struct on the target ABI |

## B12 — Pointer ownership at FFI signatures

Pointer types at FFI signatures are **non-owning by default**.  A
`*UInt8` parameter is borrowed for the duration of the call; the
called C function must not retain it beyond that call.  To transfer
ownership across the boundary, the wrapper must spell the transfer
out — e.g. `String.from_raw(ptr, len)` to take ownership of a
returned C buffer, or `Box.from_raw(ptr)` for a single allocation.
Riven does not insert any implicit drop for raw pointers.

---

## Pin tests

| Behaviour | Test fn                                       | File             |
|-----------|-----------------------------------------------|------------------|
| B1        | `parse_unsafe_block` + `parse_unsafe_block_with_statements` | `phase7_ffi.rs` |
| B2        | `parse_null_literal` + `parse_raw_pointer_type` + `parse_raw_mut_pointer_type` | `phase7_ffi.rs` |
| B3        | `parse_lib_block` + `parse_lib_block_with_link_attr` | `phase7_ffi.rs` |
| B4        | `parse_extern_block`                          | `phase7_ffi.rs`  |
| B5        | `parse_ffi_void_return`                       | `phase7_ffi.rs`  |
| B6        | `parse_ffi_multiple_params`                   | `phase7_ffi.rs`  |
| B7        | (variadic FFI parser pin — to land alongside B7 implementation) | `phase7_ffi.rs` |
| B8        | `parse_repr_c_struct` + `c_struct_layout_matches_regular` | `phase7_ffi.rs` |
| B9        | `parse_derive_attr_on_struct` + `parse_derive_attr_on_class` + `parse_derive_attr_on_enum` | `phase7_ffi.rs` |
| B10       | `parse_inbody_derive_on_class` + `parse_inbody_derive_on_enum` | `phase7_ffi.rs` |
| B11 ptr   | `raw_pointer_layout`                          | `phase7_ffi.rs`  |
| B11 packed| `packed_struct_layout`                        | `phase7_ffi.rs`  |
| B11 trans | `transparent_struct_layout` + `transparent_struct_rejects_multiple_fields` | `phase7_ffi.rs` |

<!-- TODO(migration): pin-test fn names still mention `null` / `mut` / `extern` / `derive` / `repr_c`. Internal Rust identifiers — rename when in scope. -->

---

## Out of scope (v2)

- Calling C from Riven at runtime — Phase 7 covers parser + layout +
  declaration; the call-trampoline pin tests still need to land
  (tracked in the Phase 7 prompt).
- C++ name mangling.
- Bit-field / `_Atomic` layout matching.
- Calling Riven from C (would need a stable Riven C header).
