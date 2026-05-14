# In-Body Directives

Riven attaches metadata to a declaration **inside the body of the
thing it modifies** — the same way `private`, `attr_accessor`, and
`include` live in Ruby class bodies. Three in-body directives plus
the `!` macro suffix convention on method names.

| Directive | Where it goes | What it does |
|-----------|--------------|---------------|
| `layout`  | At the top of a `struct` body | Pins memory representation |
| `inline`  | Modifier on a `def`, or standalone `inline :name` | Inlining hint |
| `include` | Inside a type body | Adopts a mixin's contract + defaults |
| `!` suffix | On a method name | Marks the method as macro-aware / can-panic |

The trait methods you'd otherwise hand-list are **synthesized
automatically** — Riven has no derive directive at all. The four
sections below walk auto-synthesis, layout, inline, and the `!`
convention. The `include` directive is covered alongside mixins —
see [Chapter 8](08-traits.md).

---

## 1. Automatic mixin synthesis

The compiler synthesizes a fixed set of common mixins for any
class, struct, or enum whose fields structurally support them. No
declaration is needed:

```riven
struct Point
  x: Int
  y: Int
end

let p = Point.new(1, 2)
puts "#{p}"           # auto-synthesized Debug
puts p == Point.new(1, 2)   # auto-synthesized PartialEq
let copy = p          # auto-synthesized Copy (struct with all-Copy fields)
```

The synthesized set, and the field-level requirement that triggers
synthesis:

| Mixin       | Auto-synth rule                                            |
|-------------|------------------------------------------------------------|
| `Debug`     | Always — formats as `TypeName(field=value, ...)`.          |
| `Clone`     | When every field is `Clone`.                               |
| `Eq` / `PartialEq` | When every field is `Eq`. Field-wise `==`.          |
| `Hash`      | When every field is `Hash`. FNV mixer over fields in source order. |
| `Default`   | When every field has a default value.                      |
| `Ord` / `PartialOrd` | When every field is `Ord`. Lexicographic by source order. |

`Copy` is the special case — it's structural and ownership-affecting:

- A `struct` whose every field is `Copy` is itself `Copy`.
- A `struct` with any non-`Copy` field is not `Copy`.
- A `class` is never `Copy` (reference semantics).

### Overriding a synthesis

Define the method yourself. Your definition wins; no auto-synth
runs for that method.

```riven
struct Point
  x: Int
  y: Int

  def to_debug -> String              # overrides the synthesized Debug
    "(#{self.x}, #{self.y})"
  end
end
```

### When a field doesn't support the mixin

If a struct contains a non-`Hash` field, the struct does not
implement `Hash`. The error appears at the **use site** — for
example, when you try to use the value as a `Map` key — and names
the offending field and the missing mixin:

```
error[E-USE-HASH]: cannot use `Point` as a `Map` key
  `Point.handle: Resource` is not Hash
help: implement Hash for Point manually, or wrap the offending field
```

---

## 2. The `layout` directive

A `struct` body may carry a `layout` directive at the top of the
body. Three forms exist.

### `layout c`

```riven
struct Point
  layout c
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

### `layout packed`

```riven
struct Header
  layout packed
  kind: UInt8
  flags: UInt32
  length: UInt64
end
```

Removes inter-field padding. Layout is byte-by-byte from the
declaration order. Useful for binary protocols.

**Caveat:** packed values cannot be borrowed mutably through `&mut`
in many cases (alignment violation). Read fields by value or copy
out first.

### `layout transparent`

```riven
struct UserId
  layout transparent
  inner: Int
end
```

Single-field newtype guaranteed to have the **same layout** as the
inner type. Useful for type-level distinctions that vanish at the
ABI layer.

Layout transparent on a multi-field struct is rejected
(`E-LAYOUT-TRANSPARENT-MULTI`).

### Default (no directive)

Without a `layout` directive, the compiler may reorder fields for
size optimisation — declaration order is **not** guaranteed.

---

## 3. The `inline` modifier

`inline` is an inlining hint. Two forms:

**Modifier on a `def`:**

```riven
inline def fast_path(x: Int) -> Int
  x * 2 + 1
end
```

**Standalone directive naming a previously-defined method:**

```riven
def fast_path(x: Int) -> Int
  x * 2 + 1
end
inline :fast_path
```

`inline` is a hint, not a guarantee. The codegen backend treats it
as `alwaysinline` when LLVM is wired (v2); Cranelift currently
ignores it.

---

## 4. The `!` macro suffix convention

Method calls ending in `!` mark the call as **macro-aware** in two
ways:

1. Lexical convention: signals "this can panic" or "this expands to
   a special form".
2. Argument-list parsing: macro-aware calls accept forms the regular
   call parser would reject (e.g. format strings inline).

Today's `!` methods:

| Method                 | Origin                              |
|------------------------|--------------------------------------|
| `panic!(msg)`          | prelude — `riven_panic`              |
| `expect!(msg)`         | `Option` / `Result`                  |
| `unwrap!()`            | `Option` / `Result`                  |
| `array![...]`          | prelude — growable array literal     |
| `map!{...}`            | prelude — map literal                |
| `set!{...}`            | prelude — set literal                |
| `Mutex.lock!()`        | `std.sync` (Phase 4 runtime)         |
| `JoinHandle.join!()`   | `std.sync` (Phase 4 runtime)         |

Don't define your own `!`-suffix methods — the convention is
reserved for compiler-aware forms.

---

## 5. Why in-body directives

The Ruby world settled long ago on putting metadata next to the
field list: `attr_accessor :name`, `include Comparable`,
`validates :email`, `private`. Riven follows the same pattern.
Everything that describes a type lives **inside its body**, in
source order.

This means:

- A type's full contract is visible in one place — no separate
  prefix-attribute syntax that wraps the type from outside.
- Adding metadata doesn't change the type's outer surface.
- The directives compose with each other and with visibility
  markers (`public`, `private`, `protected`).

---

## 6. Reading what each directive does

When you see one of these directives in source:

1. **`layout c` / `layout packed` / `layout transparent`** — pins
   field layout.
2. **`inline def ...` / `inline :name`** — inlining hint;
   ignored by Cranelift, mandatory for LLVM in v2.
3. **`include Mixin`** — adopts the mixin's contract and any
   default methods. Cross-link: [Chapter 8](08-traits.md).

For trait-method synthesis (`to_debug`, `clone`, `==`, etc.) there
is no directive — the compiler synthesizes from field types
automatically (§1).

---

**Next:** [Chapter 24 — Async Preview (typeck only)](24-async-preview.md)
for the staged async surface, or browse the
[spec index](../specs/README.md) for every formal contract.
