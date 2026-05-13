# Attributes

> **See also:** [Spec — derive](../specs/traits/derive.spec.md),
> [Spec — FFI](../specs/codegen/ffi.spec.md) for the layout +
> linkage attributes.

Riven attributes attach metadata to declarations.  Three categories
ship today:

1. **Derive attributes** — `@[derive(Debug, Clone, PartialEq)]` or the
   in-body `derive Debug, Clone` form.
2. **Layout attributes** — `@[repr(C)]`, `@[repr(packed)]`,
   `@[repr(transparent)]`.
3. **Linkage attributes** — `@[link(name = "c")]`, plus the
   in-development `@[inline]` hint.

This chapter walks each.

---

## 1. Derive attributes

The compiler can synthesise standard trait impls from the type
declaration.  Two forms exist:

**Attribute form:**

```riven
@[derive(Debug, Clone, PartialEq)]
struct Point
  x: Int
  y: Int
end
```

**In-body form:**

```riven
struct Point
  x: Int
  y: Int

  derive Debug, Clone, PartialEq
end
```

Both lower to the same AST node (`StructDef.derive_traits`).  Pick
whichever fits the surrounding code.  The attribute form is more
discoverable from a one-line glance; the in-body form keeps related
metadata next to the field list.

### Available derives

| Trait        | What it synthesises                                |
|--------------|-----------------------------------------------------|
| `Debug`      | `T_to_debug(self) -> String` for `"#{x:?}"` interp  |
| `Clone`      | `T.clone(&self) -> T` deep copy                     |
| `Copy`       | Marker; suppresses move-out diagnostics             |
| `PartialEq`  | Field-wise `==`                                     |
| `Eq`         | Marker (requires `PartialEq` + every field `Eq`)    |
| `Hash`       | `Hash` impl using the FNV mixer                     |
| `Default`    | `T::default()` static method                        |
| `Ord`        | Field-wise lexicographic ordering                   |
| `PartialOrd` | Same as `Ord` but partial                           |

Mixing derives that have unmet bounds emits diagnostics:

| Code   | Trigger                                                |
|--------|--------------------------------------------------------|
| E0607  | `derive` on a `def` or other invalid target            |
| E0610  | `derive Clone` on struct with non-`Clone` field        |
| E0611  | `derive Clone` on enum with non-`Clone` payload        |
| E0613  | `derive PartialEq` on struct with non-`Eq` field       |
| E0615  | `derive Hash` on struct with non-`Hash` field          |
| E0616  | `derive Default` on empty enum                         |
| E0617  | `derive Ord` on struct with non-`Ord` field            |
| E0618  | `derive PartialOrd` on struct with non-`PartialOrd` field |

The full derive surface + every diagnostic is spec'd in
[`docs/specs/traits/derive.spec.md`](../specs/traits/derive.spec.md).

---

## 2. Layout attributes

Layout attributes pin the memory representation of `struct` / `class`
fields.

### `@[repr(C)]`

```riven
@[repr(C)]
struct Point
  x: Int
  y: Int
end
```

Guarantees:
- Fields in source order.
- Native alignment for each field.
- No reordering for size optimisation.
- ABI-compatible with a C `struct { int64_t x; int64_t y; }`.

Use when passing values across an FFI boundary or when matching an
existing C header layout.

### `@[repr(packed)]`

```riven
@[repr(packed)]
struct Header
  kind: UInt8
  flags: UInt32
  length: UInt64
end
```

Removes inter-field padding.  Layout is byte-by-byte from the
declaration order.  Useful for binary protocols.

**Caveat:** packed values cannot be borrowed mutably through `&mut`
in many cases (alignment violation).  Read fields by value or copy
out first.

### `@[repr(transparent)]`

```riven
@[repr(transparent)]
struct UserId(Int)
```

Single-field newtype guaranteed to have the **same layout** as the
inner type.  Useful for type-level distinctions that vanish at the
ABI layer.

The parser rejects `@[repr(transparent)]` on multi-field structs
(`E_REPR_TRANSPARENT_NEEDS_SINGLE_FIELD`).

---

## 3. Linkage attributes

### `@[link(name = "libfoo")]`

Used on `lib` blocks to pin the linker name.

```riven
@[link(name = "c")]
lib "c"
  fn malloc(n: USize) -> *mut Void
  fn free(p: *mut Void)
end
```

The argument is the library name *without* the `lib` prefix or `.so`
/ `.dylib` suffix — the linker adds those.

### `@[inline]`

Reserved for the compiler.  The parser accepts it; codegen currently
ignores it (the optimiser inlines small fns anyway).  Will become a
mandatory hint in v2 when LLVM `alwaysinline` is wired.

---

## 4. Attribute syntax rules

- Attributes attach to the **next item** in source order.
- Multiple attributes stack: `@[repr(C)] @[derive(Debug)] struct ...`.
- Whitespace between attribute and item is allowed.
- Argument forms: bare name (`@[inline]`), name-value (`@[link(name = "c")]`),
  parenthesised list (`@[derive(Debug, Clone)]`).
- Arguments containing parens, commas, or strings round-trip through
  the lexer unchanged (the parser's `parse_attr_arg` is lenient).

---

## 5. The `!` macro suffix convention

Method calls ending in `!` mark the call as **macro-like** in two
ways:

1. Lexical convention: signals "this can panic" or "this expands to
   special form".
2. Argument-list parsing: `parse_macro_call_args` accepts forms the
   regular call parser would reject (e.g. format strings inline).

Today's `!` methods:

| Method                 | Origin                              |
|------------------------|--------------------------------------|
| `panic!(msg)`          | prelude — `riven_panic`              |
| `expect!(msg)`         | `Option` / `Result`                  |
| `unwrap!()`            | `Option` / `Result`                  |
| `Mutex.lock!()`        | `std::sync` (Phase 4 runtime)        |
| `JoinHandle.join!()`   | `std::sync` (Phase 4 runtime)        |

Don't define your own `!`-suffix methods — the convention is reserved
for compiler-aware forms.

---

## 6. Reading what attributes do

When you see `@[X]` in a source file:

1. **Derive trait names** (`Debug`, `Clone`, etc.) — synthesise the
   listed traits.  Cross-link: [`traits/derive.spec.md`](../specs/traits/derive.spec.md).
2. **`repr(...)`** — pin field layout.  Cross-link:
   [`codegen/ffi.spec.md`](../specs/codegen/ffi.spec.md) B7-B10.
3. **`link(...)`** — pin linker name on an external library.
4. **Anything else** — parser accepts the syntax but no pass
   currently consumes it.  Lint warning may surface in a future
   release.

---

**Next:** [Chapter 24 — Async Preview (typeck only)](24-async-preview.md)
for the staged async surface, or browse the
[spec index](../specs/README.md) for every formal contract.
