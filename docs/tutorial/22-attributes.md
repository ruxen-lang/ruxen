# In-Body Directives

Sometimes you want to tell the compiler something extra about a type — how to lay it out in memory so it matches a C header, whether a small function should be inlined for speed, which behaviour to include from a mixin. These are **directives**: small pieces of metadata you write *inside* a type's body, the same way Ruby code uses `attr_accessor`, `private`, and `include` inside a class.

This chapter covers the directives you'll meet most often: implicit mixins (the ones you get for free), the `layout` directive for memory representation, the `inline` modifier for performance, and the `!` suffix convention on method names.

---

## 1. Your first directive — `include`

Here's a struct and a quick check that it works:

```ruxen
struct Point
  x: Int
  y: Int
end

def main
  let p = Point { x: 1, y: 2 }
  puts "#{p:?}"
  puts "#{p == Point { x: 1, y: 2 }}"
end
```

Run:

```bash
ruxen run point.rx
```

Output:

```
Point { x: 1, y: 2 }
true
```

Notice you didn't write `include Debug` or `include PartialEq`. The compiler **implicitly** included them for you because every field of `Point` already supports them. This auto-inclusion is the workhorse of the directive system — most of the time, you get the common behaviours for free and never write anything.

## 2. The implicit-include set

When you declare a class, struct, or enum, the compiler checks each of these mixins. If every field structurally supports it, the mixin is included automatically:

| Mixin              | Included when…                                                  |
|--------------------|------------------------------------------------------------------|
| `Debug`            | Always. Renders as `TypeName { field: value, ... }`.            |
| `Clone`            | Every field is `Clone`.                                          |
| `Eq` / `PartialEq` | Every field is `Eq`. Field-by-field `==`.                       |
| `Hashable`         | Every field is `Hashable`.                                       |
| `Default`          | Every field has a default value.                                 |
| `Ord` / `PartialOrd` | Every field is `Ord`. Lexicographic in source order.          |
| `Send`             | Every field is `Send`. Auto-mixin — never written by hand.       |
| `Sync`             | Every field is `Sync`. Auto-mixin — never written by hand.       |
| `Copy`             | Struct only, when every field is `Copy`. Classes are never `Copy`. |

`Send` and `Sync` are about whether a type is safe to move between threads or share across them; you almost never write them yourself. If you need to opt out (because your type contains a raw pointer or something the compiler can't verify), write `exclude Send` or `exclude Sync` in the type body.

### Writing the include explicitly

If you want documentation clarity — or to fail loudly if a field change ever broke the inclusion — write the include yourself:

```ruxen
struct Point
  x: Int
  y: Int
  include Debug, Clone, Eq, Hashable
end
```

The behaviour is identical; the value is that anyone reading the type sees the contract at a glance.

### Overriding the synthesised default

Define the method yourself, and your definition wins:

```ruxen
struct Point
  x: Int
  y: Int

  def to_debug -> String
    "(#{self.x}, #{self.y})"
  end
end
```

Now `"#{p:?}"` prints `(1, 2)` instead of `Point { x: 1, y: 2 }`.

### When a field doesn't support a mixin

If `Point` had a field whose type isn't `Hashable`, the implicit `Hashable` include wouldn't fire. You'd find out at the **use site** — typically when you try to use a `Point` as a `Map` key — with an error that names the offending field:

```
error: cannot use `Point` as a `Map` key
  `Point.handle: Resource` is not Hashable
help: implement Hashable for Point manually, or wrap the offending field
```

---

## 3. The `layout` directive

By default, the compiler is free to reorder a struct's fields for the most compact size. That's almost always what you want — except when you need the layout to match a C header, a wire protocol, or a tightly-packed binary format. The `layout` directive pins the memory representation.

Three forms:

### `layout c`

```ruxen
struct Point
  layout c
  x: Int
  y: Int
end
```

Guarantees:

- Fields appear in source order.
- Each field uses its native alignment.
- No reordering for size optimisation.
- The struct is ABI-compatible with a C `struct { int64_t x; int64_t y; }`.

Use this when passing values across an FFI boundary ([Chapter 14](14-ffi.md)) or matching an existing C header.

### `layout packed`

```ruxen
struct Header
  layout packed
  kind: UInt8
  flags: UInt32
  length: UInt64
end
```

Removes padding between fields — the bytes are laid out in declaration order, back-to-back. Useful for parsing binary protocols where the wire format has no padding.

Caveat: writing through a `&var` reference to a packed field can fail an alignment check on some platforms. Read the field by value (copying it out), do your work, and write it back.

### `layout transparent`

```ruxen
struct UserId
  layout transparent
  inner: Int
end
```

For a single-field newtype-style struct, this guarantees the wrapper has the **same memory layout** as its inner type. Useful when you want a distinct type at the source level but no extra bytes at runtime.

A `layout transparent` on a multi-field struct is rejected at compile time.

### No directive

Without a `layout`, the compiler may reorder fields. Don't assume source order = memory order unless you've pinned it.

---

## 4. The `inline` modifier

`inline` is a hint to the optimiser: "consider expanding the body of this function at every call site rather than emitting a function call." Two forms:

**As a modifier on the `def`:**

```ruxen
inline def fast_path(x: Int) -> Int
  x * 2 + 1
end
```

**As a standalone directive naming a method:**

```ruxen
def fast_path(x: Int) -> Int
  x * 2 + 1
end
inline :fast_path
```

`inline` is a hint, not a guarantee — the compiler may still choose not to inline. It's most useful on tiny, hot helpers.

---

## 5. The `!` macro suffix convention

A method name ending in `!` signals two things:

1. **It may panic** (or otherwise do something dramatic) on failure.
2. **It's compiler-aware** — the parser may accept argument forms a regular call would reject (like format strings).

Today's `!`-suffix methods:

| Method                 | What it does                                |
|------------------------|---------------------------------------------|
| `panic!(msg)`          | Abort the program with `msg` on stderr      |
| `expect!(msg)`         | On `Option` / `Result`: unwrap or panic     |
| `unwrap!()`            | On `Option` / `Result`: unwrap or panic     |

The convention is reserved for compiler-aware forms — don't define your own `!`-suffix methods.

---

## 6. Why "in-body" instead of "in front of"?

A handful of languages put metadata in *front* of the thing (Java's `@Override`, Rust's `#[derive(...)]`). Ruxen takes the other path — borrowed from Ruby — and puts it *inside* the body. The win is that a type's complete contract is visible in one place: open the body, read top-to-bottom, and you see every directive, every field, and every method together.

```ruxen
struct Header
  layout c                        # memory layout
  include Debug, Clone, Eq        # behaviour contract
  kind: UInt8                     # fields
  length: UInt32
end
```

---

## 7. Common mistakes

- **Forgetting `layout c` before passing a struct to C.** Without it, the compiler may reorder the fields and your C code reads garbage. Add it the moment a struct crosses an FFI boundary.
- **Expecting `layout packed` to be free.** It saves bytes but every unaligned read costs cycles, and writing through `&var` can fail outright. Use it only for serialisation buffers, not for general-purpose data.
- **Treating `inline` as a guarantee.** It's a hint. If you measured and your hot loop didn't inline, look at smaller fixes (shorter body, fewer parameters) before reaching for it.
- **Trying to use `derive` syntax.** Ruxen has no `derive` — the implicit-include rules cover what `derive Debug, Clone` would do in other languages, and the loud form is `include Debug, Clone` inside the type body.

> **Try it:** add a `Resource` field to `Point` that doesn't implement `Hashable`, then try to use the point as a `Map` key. Read the resulting compile error carefully — it tells you exactly which field is the problem.

---

## Recap

- Directives go **inside** the type's body, right next to the fields.
- The compiler implicitly includes `Debug`, `Clone`, `Eq`, `Hashable`, `Default`, `Ord`, `Send`, `Sync` when fields allow.
- `layout c`, `layout packed`, `layout transparent` pin memory representation.
- `inline def f` or `inline :f` hints the optimiser.
- `!`-suffix methods signal compiler-aware, possibly-panicking operations.

**Next:** [Chapter 23 — Implementing Iterator for Your Own Type](23-implementing-iterator.md).
