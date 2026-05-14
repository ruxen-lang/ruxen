# String Formatting and Interpolation

> **See also:** [Spec — std.fmt](../specs/stdlib/fmt.spec.md).  This
> tutorial is the user-facing companion; the spec is the source of
> truth for the compiler's behaviour.

Riven gives you three layers for turning values into strings:

1. **Interpolation** — `"hello #{name}"` inline in any string literal.
2. **The `Display` mixin** — how *your* types render in interpolation.
3. **The `Debug` mixin** — `"#{x:?}"` for diagnostic output, usually
   via `derive Debug`.

---

## 1. Basic interpolation

Wrap any expression in `#{...}` inside a double-quoted string:

```riven
def main
  let name = "world"
  let n = 42
  puts "hello, #{name}! count = #{n}"
end
```

Output:

```
hello, world! count = 42
```

The compiler routes every interpolated expression through the
`Display.fmt` dispatch path:

```text
fmt = Formatter.new()
{T}_fmt(value, fmt)          # T_fmt = Display fmt for the value's type
buf = fmt.buffer()           # consumes the Formatter, returns String
```

For primitives (`Char`, `Int`, `Float`, `Bool`, `String`) the compiler
synthesises the `T_fmt` function automatically.  For your own types,
include the `Display` mixin and provide a `fmt` method (see §4).

## 2. Debug interpolation: `"#{x:?}"`

The `:?` spec routes through `Debug` instead of `Display`.  Any type
that opts into `derive Debug` gets a compiler-generated `T_to_debug`
function:

```riven
struct Point
  x: Int
  y: Int

end

def main
  let p = Point { x: 1, y: 2 }
  puts "p = #{p:?}"
end
```

Output:

```
p = Point { x: 1, y: 2 }
```

Enums work the same way:

```riven
enum Color
  Red
  Green
  Blue
  Custom { r: Int, g: Int, b: Int }

end

def main
  puts "#{Color.Red:?}"
  puts "#{Color.Custom { r: 10, g: 20, b: 30 }:?}"
end
```

Output:

```
Red
Custom { r: 10, g: 20, b: 30 }
```

## 3. Format specs: width, alignment, fill, precision

The grammar inside `#{...}` mirrors Rust's:

```
"#{<expr>:[<fill><align>][<width>][.<precision>][?]}"
```

| Field        | Form         | Example       | Effect                          |
|--------------|--------------|---------------|---------------------------------|
| `fill align` | `<c>[<>^]`   | `*<`          | Pad char + side                 |
| `align`      | `[<>^]`      | `>`           | Pad side (default = right for numerics) |
| `width`      | digits       | `10`          | Minimum total width             |
| `.precision` | `.` + digits | `.2`          | Float decimals; String char cap |
| `?`          | literal `?`  | `?`           | Debug instead of Display        |

### 3.1 Width and alignment

```riven
def main
  let n: Int = 42
  puts "[#{n:>5}]"      # right-align (default for numerics)
  puts "[#{n:<5}]"      # left-align
  puts "[#{n:^6}]"      # center-align
end
```

Output:

```
[   42]
[42   ]
[  42  ]
```

When the value is wider than the requested width, no truncation
happens — the value prints in full.

### 3.2 Custom fill character

Any single character before the alignment flag becomes the pad
char:

```riven
def main
  let n: Int = 7
  puts "[#{n:*<5}]"     # left-align, fill with `*`
  puts "[#{n:0>4}]"     # zero-pad on the left (NOT the same as `:04` Rust syntax)
end
```

Output:

```
[7****]
[0007]
```

Non-ASCII fill codepoints fall back to space in v1.

### 3.3 Float precision

Precision (`.N`) is the number of decimal digits for floats:

```riven
def main
  let pi: Float = 3.14159
  puts "pi = #{pi:.2}"        # 2 decimal places
  puts "pi = #{pi:.5}"        # 5 decimal places
end
```

Output:

```
pi = 3.14
pi = 3.14159
```

Internally this routes through `Float_to_string_prec(value, prec)`
which uses `snprintf("%.*f", prec, value)`.

### 3.4 String precision (truncate)

For strings, precision is the maximum number of **characters**
(UTF-8 codepoints) — boundary-safe:

```riven
def main
  let s: String = String.from("hello world")
  puts "[#{s:.5}]"
end
```

Output:

```
[hello]
```

For `Int`, `Bool`, and `Char`, precision is **ignored** — matches
Rust's `min_const_generics`-era semantics.

### 3.5 Composing width + precision

The two compose: precision runs first (shortening the value), then
width pads the result.

```riven
def main
  let pi: Float = 3.14159
  puts "[#{pi:>8.2}]"
end
```

Output:

```
[    3.14]
```

Read as "right-align, total width 8, 2 decimal places".

## 4. Implementing `Display` for your own type

```riven
class Money
  cents: Int

  def init(@cents: Int)
  end

  include Display

  def fmt(f: &mut Formatter) -> Result[(), FmtError]
    let _ = f.write_str("$")
    f.write_str("#{self.cents}")
  end
end

def main
  let m = Money.new(4250)
  puts "price: #{m}"
end
```

Output:

```
price: $4250
```

Things to note:

- `fmt` takes `&mut Formatter` explicitly; `self` is the implicit
  reading receiver, same as elsewhere in Riven.
- `f.write_str` returns `Result[(), FmtError]`.  Returning that
  result directly from `fmt` is idiomatic; `let _ = ...` discards
  earlier intermediate calls.
- Inside `fmt` you can use interpolation `"#{...}"` itself — that
  inner interpolation routes through `Display.fmt` again (`Int_fmt`
  in this case).

### 4.1 User `Display` plus format specs

When the call site adds a spec — `"#{m:>10}"` — width / align / fill
still apply.  The compiler builds the formatter with the spec, your
`fmt` body writes into the buffer, and `Formatter.buffer()` applies
padding at finalize time.  Precision is type-specific and your
`fmt` body sees the value via `f.precision()` if it wants to react.

## 5. `Formatter` helpers

Inside a class that includes `Display`, the formatter exposes:

| Method                | Returns                | Notes                          |
|-----------------------|------------------------|--------------------------------|
| `f.write_str(&str)`   | `Result[(), FmtError]` | Append a string                |
| `f.write_char(Char)`  | `Result[(), FmtError]` | Append a single codepoint      |
| `f.buffer()`          | `String`               | Consumes `f`; not for user use |
| `f.len()`             | `Int`                  | Bytes accumulated so far       |
| `f.width()`           | `Option[Int]`          | The spec's width, if any       |
| `f.precision()`       | `Option[Int]`          | The spec's precision, if any   |
| `f.align()`           | `Char`                 | `<`, `>`, `^`, or space        |
| `f.fill()`            | `Char`                 | Fill char (default `' '`)      |

Don't call `f.buffer()` yourself inside `fmt` — that's how the
compiler ends the dispatch.  Use `write_str` / `write_char` for
output and let the compiler call `buffer()` at the call site.

## 6. Common pitfalls

- **`derive Debug` only.** A bare `"#{x}"` for a type that derives
  `Debug` but does **not** include `Display` falls back to the
  Debug path (so you still see something useful).  Once the type
  includes `Display`, the bare form picks Display automatically.
- **Width on `:?`.** The Debug path bypasses the Formatter today, so
  `"#{x:?}"` ignores width / align / fill.  This is a documented v1
  limitation (see [fmt.spec.md Out of scope](../specs/stdlib/fmt.spec.md)).
- **Sign / radix flags.** `:x`, `:X`, `:b`, `:o`, `:e`, leading `+`,
  `#` alternate form, and `0` zero-pad — none of these are parsed
  yet.  Use a `.to_radix(...)` method or write a wrapper type for
  now.

## 7. Where this lives in the compiler

The pipeline is:

```
Lexer captures FormatSpec (width / align / fill / precision / debug)
   ↓
HIR carries it on HirInterpolationPart::Expr { expr, spec }
   ↓
MIR lower_interpolation emits Formatter_new_with_spec(...) + T_fmt + Formatter_buffer
   ↓
Runtime applies width/align/fill at Formatter_buffer finalize
   ↓
Float_fmt + String_fmt read precision via Formatter_precision
   ↓
Output goes to puts / println / String binding
```

Every step is pin-tested.  See the [spec's Pin tests table](../specs/stdlib/fmt.spec.md#pin-tests)
for the exact test functions that exercise each behaviour.

---

**Next:** [Chapter 14 — Foreign Function Interface](14-ffi.md) if you
want to call into C libraries; [Chapter 15 — Unsafe](15-unsafe.md) for
the unsafe-block surface that supports it.
