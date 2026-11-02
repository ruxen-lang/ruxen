# In-Body Directives

Sometimes you want to tell the compiler something extra about a type — how to lay it out in memory so it matches a C header, whether a small method should be inlined for speed, which behaviour to include from a mixin. These are **directives**: small pieces of metadata you write *inside* a type's body, the same way Ruby code uses `attr_accessor`, `private`, and `include` inside a class.

This chapter covers the directives you'll meet most often: implicit mixins (the ones you get for free), the `layout` directive for memory representation, the `inline` modifier for performance, and the `!` suffix convention on method names.

---

## 1. Your first directive — `include`

Here's a class and a quick check that it works. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class Point
  x: Int
  y: Int

  def init(@x: Int, @y: Int)
  end
end

pt = Point.new(1, 2)
puts pt
puts pt == Point.new(1, 2)
```

Run:

```sh
ruxen run point.rx
```

Output:

```text
Point { x: 1, y: 2 }
true
```

Notice you didn't write a `to_s` or an `==`. The compiler **implicitly** gave `Point` a default rendering and a field-by-field `==` because every field of `Point` already supports them. This auto-inclusion is the workhorse of the directive system [implemented] — most of the time, you get the common behaviours for free and never write anything.

Ruby's default rendering, `#<Point x=1, y=2>`, is [planned](../../TODO.md#t-d9); today the default prints the `Point { x: 1, y: 2 }` shape above.

## 2. The implicit-include set

When you declare a class, the compiler checks each of these behaviours. If every field structurally supports it, it is included automatically. Several still carry their Rust mixin names, which are [deprecated]; the table names the Ruby method each one becomes.

| What you get | Included when… | Status |
|---|---|---|
| Default rendering for `puts` and `#{x:?}` | Always. Renders as `TypeName { field: value, ... }`. | [implemented]; the `Debug` name is [deprecated], replaced by `inspect` [planned](../../TODO.md#t-d9) |
| `clone` | Every field can be cloned. | [implemented] (`Clone` stays as an ownership marker) |
| Field-by-field `==` | Every field has an `==`. | [implemented]; the `PartialEq` / `Eq` names are [deprecated], replaced by `==`. A structural `==` for value types is [planned](../../TODO.md#t-d8) |
| Hash-key identity | Every field can be hashed. | `Hashable` and `hash_code` [deprecated], replaced by Ruby's `hash` and `eql?` [planned](../../TODO.md#t-d8) |
| `Type.default` | Every field has a default value. | [deprecated], replace with arguments with defaults or `def initialize` with no parameters [planned](../../TODO.md#t-initialize) |
| Ordering, lexicographic in source order | Every field can be ordered. | The `Ord` / `PartialOrd` names are [deprecated], replace with `include Comparable` and `def <=>` [implemented] |
| `Send` | Every field is `Send`. Auto-mixin — never written by hand. | [implemented] |
| `Sync` | Every field is `Sync`. Auto-mixin — never written by hand. | [implemented] |
| `Copy` | A `struct` whose every field is `Copy`. Classes are never `Copy`. | [deprecated] with `struct`; a `Data` value whose fields are all Copy will be Copy [planned](../../TODO.md#t-data-define) |

`Send` and `Sync` are about whether a type is safe to move between threads or share across them; you almost never write them yourself. If you need to opt out (because your type contains a raw pointer or something the compiler can't verify), write `include !Send` or `include !Sync` in the type body [implemented]. A single `exclude Send` spelling is [planned](../../TODO.md#t-exclude-send).

### Writing the include explicitly

If you want documentation clarity — or to fail loudly if a field change ever broke the inclusion — you can write the include yourself. The Rust mixin names are [deprecated] (replace them with the Ruby methods in the table above), and listing several modules in one `include A, B` is [planned](../../TODO.md#t-include-many), so today it is one `include` per line:

```ruby
class Point
  include Clone
  include PartialEq
  x: Int
  y: Int

  def init(@x: Int, @y: Int)
  end
end
```

The behaviour is identical; the value is that anyone reading the type sees the contract at a glance.

### Overriding the synthesised default

Define the method yourself, and your definition wins. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class Point
  x: Int
  y: Int

  def init(@x: Int, @y: Int)
  end

  def to_s -> String = "(#{@x}, #{@y})"
end

puts Point.new(1, 2)
```

Now `puts` prints `(1, 2)` instead of `Point { x: 1, y: 2 }`. Interpolating the point directly (`"#{pt}"`) still prints the default today [known issue](../../TODO.md#t-interp-to-s). A user `def inspect` for `p` is [planned](../../TODO.md#t-d9).

### When a field doesn't support a mixin

If `Point` had a field whose type can't be hashed, the implicit hashing include wouldn't fire. You'd find out at the **use site** — typically when you try to use a `Point` as a `Hash` key — with a compile error that names the offending field [implemented].

---

## 3. The `layout` directive

By default, the compiler is free to reorder a type's fields for the most compact size. That's almost always what you want — except when you need the layout to match a C header, a wire protocol, or a tightly-packed binary format. The `layout` directive pins the memory representation.

Three forms:

### `layout c`

Example [implemented]:

```ruby
class Point
  layout c
  x: Int
  y: Int
end
```

Guarantees:

- Fields appear in source order.
- Each field uses its native alignment.
- No reordering for size optimisation.
- The type is ABI-compatible with a C `struct { int64_t x; int64_t y; }`.

Use this when passing values across an FFI boundary ([Chapter 14](14-ffi.md)) or matching an existing C header. `layout c` is the first line of the class body.

### `layout packed`

A class body accepts only `layout c`. The other two layouts are written in a `struct` body, which is [deprecated] (replace with `Data.define` or `Struct.new` [planned](../../TODO.md#t-data-define)); `layout packed` and `layout transparent` hit a `struct` type-check failure today [known issue](../../TODO.md#t-struct-typeck):

```ruby
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

Also a `struct` form, [deprecated] like `layout packed` above, with the same [known issue](../../TODO.md#t-struct-typeck):

```ruby
struct UserId
  layout transparent
  inner: Int
end
```

For a single-field wrapper, this guarantees the wrapper has the **same memory layout** as its inner type. Useful when you want a distinct type at the source level but no extra bytes at runtime.

A `layout transparent` on a multi-field struct is rejected at compile time.

### No directive

Without a `layout`, the compiler may reorder fields. Don't assume source order = memory order unless you've pinned it.

---

## 4. The `inline` modifier

`inline` is a hint to the optimiser: "consider expanding the body of this method at every call site rather than emitting a call." Write it as a modifier on a `def` in a class body. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class Scaler
  factor: Int

  def init(@factor: Int)
  end

  inline def apply(x: Int) -> Int
    x * @factor + 1
  end
end

puts Scaler.new(2).apply(3)
```

`inline def` on a top-level method is [planned](../../TODO.md#t-toplevel-stmts). The standalone form `inline :fast_path` from earlier drafts does not parse; write `inline def`.

`inline` is a hint, not a guarantee — the compiler may still choose not to inline. It's most useful on tiny, hot helpers.

---

## 5. The `!` suffix convention

As in Ruby, a method name ending in `!` marks the "dangerous" variant: it may abort (or otherwise do something dramatic) on failure.

The built-in `!`-suffix forms:

| Method | What it does | Status |
|---|---|---|
| `panic!(msg)` | Abort the program with `msg` on stderr | [deprecated], replace with `abort "msg"` [implemented] or `raise "msg"` [planned](../../TODO.md#t-exceptions) |
| `expect!(msg)` | On an optional value or a `Result`: unwrap or abort | [deprecated], replace with `x \|\| default`, `fetch` or `case/in` [planned](../../TODO.md#t-truthiness) |
| `unwrap!` | On an optional value or a `Result`: unwrap or abort | [deprecated], replace with the same forms as `expect!` [planned](../../TODO.md#t-truthiness) |

You can also define your own Ruby-style bang methods (`def save!`, `def shout!(x: Int)`) [implemented]; they are ordinary methods called with the `!` attached. Follow the Ruby convention and use the `!` for the "dangerous" variant of a method.

---

## 6. Why "in-body" instead of "in front of"?

Some languages put metadata in *front* of the thing (Java's `@Override` annotation). Ruxen follows Ruby and puts it *inside* the body. The win is that a type's complete contract is visible in one place: open the body, read top-to-bottom, and you see every directive, every field, and every method together. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class Header
  layout c                        # memory layout
  include !Send                   # behaviour contract
  kind: UInt8                     # fields
  length: UInt32

  def init(@kind: UInt8, @length: UInt32)
  end
end
```

The `@[...]` attribute spelling in front of an item is [retired] (E0607).

---

## 7. Common mistakes

- **Forgetting `layout c` before passing a type to C.** Without it, the compiler may reorder the fields and your C code reads garbage. Add it the moment a type crosses an FFI boundary.
- **Expecting `layout packed` to be free.** It saves bytes but every unaligned read costs cycles, and writing through `&var` can fail outright. Use it only for serialisation buffers, not for general-purpose data.
- **Treating `inline` as a guarantee.** It's a hint. If you measured and your hot loop didn't inline, look at smaller fixes (shorter body, fewer parameters) before reaching for it.
- **Looking for an annotation to opt in to a default rendering or `clone`.** There is none — the implicit-include rules cover it, and the loud form is an `include` line inside the type body.

> **Try it:** add a field to `Point` whose type can't be hashed, then try to use the point as a `Hash` key. Read the resulting compile error carefully — it tells you which field is the problem.

---

## Recap

- Directives go **inside** the type's body, right next to the fields.
- The compiler implicitly gives a type a default rendering, `clone`, `==`, hashing, ordering, `Send` and `Sync` when its fields allow [implemented]; the Rust mixin names for them are [deprecated].
- `layout c` pins memory representation on a class [implemented]; `layout packed` and `layout transparent` live in [deprecated] `struct` bodies.
- `inline def f` in a class body hints the optimiser [implemented].
- `!`-suffix methods signal the dangerous variant: the built-in ones are [deprecated], and your own bang methods follow the same Ruby convention [implemented].

**Next:** [Chapter 24 — Async](24-async.md).
