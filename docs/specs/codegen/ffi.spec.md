# Spec — Phase 7 FFI surface

**Source docs:**
[docs/requirements/](../../requirements/) (no dedicated doc yet).

**Status:** shipped Phase 7 (parser + layout; runtime trampolines).

Riven's FFI surface lets user code declare external C symbols and
opt-in unsafe blocks.  This spec covers what the parser accepts and
what the layout / repr rules guarantee.

---

## B1 — `unsafe { ... }` block parses

```riven
unsafe
  ptr.deref
end
```

The block opens a region in which raw-pointer operations are legal.

## B2 — Null literal + raw pointer types parse

`null` is a typed literal that produces a `*T` value.  Raw pointer
types are spelled `*T` (read-only) and `*mut T` (write).

## B3 — `lib "name"` declaration parses

```riven
lib "c"
  def malloc(n: USize) -> *mut Void
end
```

Declares an external shared library; nested `def` declarations expose
its symbols.  An optional `path:` / `version:` option on `lib` pins
the linker name.

## B4 — `lib "<linkname>"` block parses (C-ABI)

`lib "<linkname>"` blocks declare C-callable functions; parameter
types follow the same `*T` / `*mut T` rules as B2.

## B5 — `void` return type

FFI defs may return the void type (mapped to Riven's `Unit`).

## B6 — Multi-parameter FFI

Accepts arbitrarily many parameters, mixing scalars and pointers.

## B7 — `layout c` struct layout

`layout c` on a struct guarantees C-compatible layout (fields in
declaration order, native alignment, no reordering).

## B8 — Implicit structural mixin inclusion on struct / class / enum

Structural mixins (`Debug`, `Clone`, `Eq`, `Hash`, …) are implicitly
included when the fields support them; an explicit `include D1, D2`
inside the body is the loud form and lowers to the same AST node.

## B9 — In-body `include` clause syntax

An `include Foo, Bar` clause inside a struct/class/enum body parses
the loud form of the implicit-include rule.

## B10 — Layout invariants

| Layout                                  | Guarantee                              |
|-----------------------------------------|----------------------------------------|
| Raw pointer (`*T`)                      | 8 bytes (64-bit pointer)               |
| `layout packed` struct                  | No padding between fields              |
| `layout transparent`                    | Same layout as inner field             |
| `layout transparent` w/ multiple fields | rejected at typeck                     |
| `layout c` struct                       | Matches plain struct on the target ABI |

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
| B7        | `parse_repr_c_struct` + `c_struct_layout_matches_regular` | `phase7_ffi.rs` |
| B8        | `parse_derive_attr_on_struct` + `parse_derive_attr_on_class` + `parse_derive_attr_on_enum` | `phase7_ffi.rs` |
| B9        | `parse_inbody_derive_on_class` + `parse_inbody_derive_on_enum` | `phase7_ffi.rs` |
| B10 ptr   | `raw_pointer_layout`                          | `phase7_ffi.rs`  |
| B10 packed| `packed_struct_layout`                        | `phase7_ffi.rs`  |
| B10 trans | `transparent_struct_layout` + `transparent_struct_rejects_multiple_fields` | `phase7_ffi.rs` |

---

## Out of scope (v2)

- Calling C from Riven at runtime — Phase 7 covers parser + layout +
  declaration; the call-trampoline pin tests still need to land
  (tracked in the Phase 7 prompt).
- C++ name mangling.
- Variadic functions (`...`).
- Bit-field / `_Atomic` layout matching.
- Calling Riven from C (would need a stable Riven C header).
