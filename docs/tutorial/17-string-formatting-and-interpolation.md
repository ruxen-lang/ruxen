# String Formatting and Interpolation

Almost every program eventually needs to turn values into nicely-formatted strings — to print them, log them, or build messages for users. Ruxen gives you a single syntax for this: **string interpolation**. You drop `#{...}` into any double-quoted string, and whatever expression is inside gets converted to text and spliced in. This chapter walks through the basics, then shows how to control width, alignment, and decimal precision, and finally how to teach your own types to render themselves.

---

## 1. Your first interpolated string

Here is a complete program. Save it as `hello.rx`:

```ruxen
def main
  let name = "world"
  let n = 42
  puts "hello, #{name}! count = #{n}"
end
```

Run it:

```bash
ruxen run hello.rx
```

Output:

```
hello, world! count = 42
```

That's the whole feature: anything inside `#{...}` is evaluated as an expression and converted to a string. The built-in types (`Int`, `Float`, `Bool`, `Char`, `String`) all know how to render themselves, so they work straight away.

## 2. Interpolating any expression

The thing inside `#{...}` doesn't have to be a single variable — it can be any expression:

```ruxen
def main
  let a = 3
  let b = 4
  puts "#{a} + #{b} = #{a + b}"
end
```

Output:

```
3 + 4 = 7
```

You can call methods, index into arrays, do arithmetic — anything that produces a value.

## 3. Debug interpolation with `:?`

Sometimes you want to peek at a value's internal structure — useful for debugging. Add `:?` after the expression and Ruxen prints it in **debug form**: a programmer-readable view that shows field names, enum variants, and structure.

```ruxen
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

Every type supports `:?` automatically, so you can debug-print any value without writing extra code. Enums work the same way:

```ruxen
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

## 4. Width and alignment

Sometimes you want output to line up in columns. The bit after the colon controls **width** (minimum total characters) and **alignment** (which side gets padded):

```ruxen
def main
  let n: Int = 42
  puts "[#{n:>5}]"      # right-align in width 5
  puts "[#{n:<5}]"      # left-align in width 5
  puts "[#{n:^6}]"      # center-align in width 6
end
```

Output:

```
[   42]
[42   ]
[  42  ]
```

If the value is wider than the requested width, nothing gets cut off — it just prints in full.

### Custom fill character

Want to pad with something other than spaces? Put any single character before the alignment flag:

```ruxen
def main
  let n: Int = 7
  puts "[#{n:*<5}]"     # left-align, fill with *
  puts "[#{n:0>4}]"     # right-align, fill with 0
end
```

Output:

```
[7****]
[0007]
```

> **Try it:** change `*<5` to `-<10` and re-run. What changes?

## 5. Float precision

For floating-point numbers, `.N` means "show N digits after the decimal point":

```ruxen
def main
  let pi: Float = 3.14159
  puts "pi = #{pi:.2}"
  puts "pi = #{pi:.5}"
end
```

Output:

```
pi = 3.14
pi = 3.14159
```

## 6. String truncation

For strings, `.N` means "show at most N characters":

```ruxen
def main
  let s: String = String.from("hello world")
  puts "[#{s:.5}]"
end
```

Output:

```
[hello]
```

For `Int`, `Bool`, and `Char`, the `.N` setting is ignored — those types either fit or they don't.

## 7. Combining width and precision

You can stack them. Precision runs first; width pads the result:

```ruxen
def main
  let pi: Float = 3.14159
  puts "[#{pi:>8.2}]"
end
```

Output:

```
[    3.14]
```

Read that as: "round to 2 decimals first, then right-align inside a total width of 8."

## 8. Teaching your own type to render

Out of the box, your custom classes get debug-printing (`:?`). But for plain `"#{value}"`, you need to tell Ruxen how the value should look. This is done by including the `Display` **mixin** — a mixin is a small bundle of behaviour you opt into by writing `include MixinName` inside the type's body. (See [Chapter 8](08-mixins.md) for the full story.)

```ruxen
class Money
  cents: Int

  def init(@cents: Int)
  end

  include Display

  def fmt(f: &var Formatter) -> Result[nil, FmtError]
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

A few things to notice:

- `fmt` is the method `Display` requires. It receives a `Formatter`, which is a small object you write strings into.
- `f.write_str(...)` returns a `Result`. Inside `fmt`, the very last call's `Result` is what you return; earlier ones get tossed with `let _ = ...`.
- You can use `"#{...}"` inside `fmt` itself — it just routes through `Display.fmt` recursively for whatever you interpolate.

## 9. Width and precision with your own types

When you write `"#{m:>10}"` on a value whose type implements `Display`, the width and alignment still apply. Your `fmt` body writes the raw text; Ruxen handles padding around it.

## 10. Common mistakes

- **Forgetting the `&var` on `Formatter`.** The signature must be `fmt(f: &var Formatter) -> Result[nil, FmtError]`. If you forget `&var`, you'll get a type-mismatch error pointing at the `include Display` line.
- **Using `+` to build strings.** Ruxen doesn't have `+` on `String`. Interpolation is the idiomatic way to combine pieces. Use `"#{a}#{b}"` instead of `a + b`.
- **Expecting `:x` or `:b` for hex / binary.** Those flags don't exist yet — call a method like `.to_radix(16)` and interpolate the result instead.
- **Width on `:?`.** The debug path ignores width and alignment, so `"#{value:>10?}"` won't pad. Use `Display` if you need formatted output.

> **Try it:** add a `Display` mixin to the `Point` struct from earlier and make it render as `"(1, 2)"`. Compare the result to `"#{p:?}"`.

---

## Recap

- `"#{expr}"` interpolates any expression as a string.
- `"#{expr:?}"` prints a debug view — works on every type for free.
- Format specs control width (`:>5`), alignment (`<`, `>`, `^`), fill (`*<5`), and float / string precision (`.2`, `.5`).
- For your own types, `include Display` and write `def fmt(f: &var Formatter) -> Result[nil, FmtError]`.
- Width and precision can be combined: `:>8.2`.

**Next:** [Chapter 18 — Standard Library Tour](18-stdlib-tour.md) — a "what's in the box" walkthrough of the modules you'll reach for most often.
