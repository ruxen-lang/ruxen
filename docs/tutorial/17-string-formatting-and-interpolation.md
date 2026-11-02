# String Formatting and Interpolation

Almost every program eventually needs to turn values into nicely-formatted strings — to print them, log them, or build messages for users. Ruxen uses Ruby's syntax for this: **string interpolation**. You drop `#{...}` into any double-quoted string, and whatever expression is inside gets converted to text and spliced in. This chapter walks through the basics, then shows how to control width, alignment, and decimal precision, and finally how to teach your own types to render themselves.

---

## 1. Your first interpolated string

Here is a complete program. Save it as `hello.rx`. Example [implemented]:

```ruby
name = "world"
n = 42
puts "hello, #{name}! count = #{n}"
```

Run it:

```sh
ruxen run hello.rx
```

Output:

```text
hello, world! count = 42
```

That's the whole feature: anything inside `#{...}` is evaluated as an expression and converted to a string. The built-in types (`Int`, `Float`, `Bool`, `Char`, `String`) all know how to render themselves, so they work straight away.

## 2. Interpolating any expression

The thing inside `#{...}` doesn't have to be a single variable — it can be any expression. Example [implemented]:

```ruby
a = 3
b = 4
puts "#{a} + #{b} = #{a + b}"
```

Output:

```text
3 + 4 = 7
```

You can call methods, index into arrays, do arithmetic — anything that produces a value.

## 3. Debug output: `p` and `inspect`

Sometimes you want to peek at a value's internal structure — useful for debugging. Ruby's tool for this is `p x`, which prints `x.inspect`: a programmer-readable view. For the built-in types `p` works today [implemented]; note the quotes on the string:

```ruby
p 7
p true
p "s"
```

Output:

```text
7
true
"s"
```

For your own classes, `p x`, `x.inspect` and Ruby's default rendering `#<Point x=1, y=2>` are [planned](../../TODO.md#t-d9). Today a class without its own `to_s` prints a default view with its field names when you `puts` it. Example [implemented]; its constructor is still spelled `def init` [deprecated] until Ruby's `def initialize` lands [planned](../../TODO.md#t-initialize):

```ruby
class Point
  x: Int
  y: Int

  def init(@x: Int, @y: Int)
  end
end

pt = Point.new(1, 2)
puts pt
```

Output:

```text
Point { x: 1, y: 2 }
```

The older debug spec `:?` inside interpolation also still works. It is [deprecated], replace it with `p x` or `x.inspect` once those land for your types:

```ruby
pt = Point.new(1, 2)
puts "p = #{pt:?}"
```

Output:

```text
p = Point { x: 1, y: 2 }
```

The same spec on a value of an `enum` prints the variant name. Both `enum` and `:?` are [deprecated]; replace `enum` with symbols or `Data.define` variants ([planned](../../TODO.md#t-data-define)) and `:?` with `inspect`:

```ruby
enum Color
  Red
  Green
  Blue
end

c = Color.Blue
puts "#{c:?}"
```

Output:

```text
Blue
```

## 4. Width and alignment

Sometimes you want output to line up in columns. Ruby pads a string with `rjust` (right-align), `ljust` (left-align) and `center`, each taking a minimum width. Example [implemented]:

```ruby
n = 42
puts "[" + n.to_s.rjust(5) + "]"      # right-align in width 5
puts "[" + n.to_s.ljust(5) + "]"      # left-align in width 5
puts "[" + n.to_s.center(6) + "]"     # center-align in width 6
```

Output:

```text
[   42]
[42   ]
[  42  ]
```

If the value is wider than the requested width, nothing gets cut off — it just prints in full.

Ruby's `format("%5d", n)` and `format("%-5d", n)` are [planned](../../TODO.md#t-d9). The width and alignment specs inside interpolation (`#{n:>5}`, `#{n:<5}`, `#{n:^6}`) still work today and are [deprecated], replace them with `rjust`, `ljust`, `center` or `format`:

```ruby
n = 42
puts "[#{n:>5}]"
puts "[#{n:<5}]"
puts "[#{n:^6}]"
```

Output:

```text
[   42]
[42   ]
[  42  ]
```

### Custom fill character

Want to pad with something other than spaces? Pass the fill string as the second argument. Example [implemented]:

```ruby
n = 7
puts "[" + n.to_s.ljust(5, "*") + "]"   # left-align, fill with *
puts "[" + n.to_s.rjust(4, "0") + "]"   # right-align, fill with 0
```

Output:

```text
[7****]
[0007]
```

The fill character before the alignment flag (`#{n:*<5}`, `#{n:0>4}`) is the [deprecated] spelling of the same thing, replaced by the second argument of `ljust` / `rjust`.

> **Try it:** change `ljust(5, "*")` to `ljust(10, "-")` and re-run. What changes?

## 5. Float precision

Ruby shows N digits after the decimal point with `format("%.2f", pi)`; that is [planned](../../TODO.md#t-d9), and so is `Float#round(2)`. Today the precision spec `.N` inside interpolation does it. It is [deprecated], replace it with `format` once it lands:

```ruby
pi = 3.14159
puts "pi = #{pi:.2}"
puts "pi = #{pi:.5}"
```

Output:

```text
pi = 3.14
pi = 3.14159
```

## 6. String truncation

Ruby's `format("%.5s", s)` shows at most N characters of a string; it is [planned](../../TODO.md#t-d9). Today the `.N` spec on a `String` does it, [deprecated] like the other specs:

```ruby
s = "hello world"
puts "[#{s:.5}]"
```

Output:

```text
[hello]
```

For `Int`, `Bool`, and `Char`, the `.N` setting is ignored — those types either fit or they don't.

## 7. Combining width and precision

Ruby combines them in one directive, `format("%8.2f", pi)` [planned](../../TODO.md#t-d9). The interpolation spec stacks them too: precision runs first, width pads the result. It is [deprecated], replace it with `format`:

```ruby
pi = 3.14159
puts "[#{pi:>8.2}]"
```

Output:

```text
[    3.14]
```

Read that as: "round to 2 decimals first, then right-align inside a total width of 8."

## 8. Teaching your own type to render

Out of the box, your custom classes print the default view from section 3. To choose the text yourself, define Ruby's `to_s` method; `puts` uses it. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class Money
  cents: Int

  def init(@cents: Int)
  end

  def to_s -> String = "$#{@cents}"
end

m = Money.new(4250)
puts m
puts "price: #{m.to_s}"
```

Output:

```text
$4250
price: $4250
```

A few things to notice:

- `to_s` takes no arguments and returns a `String`. Inside it, `@cents` reads the field.
- `puts m` calls your `to_s` [implemented]. Plain interpolation `"#{m}"` should call it too, but today it still prints the default view, so the example calls `m.to_s` inside the braces [known issue](../../TODO.md#t-interp-to-s).
- The older way, `include Display` with a `def fmt(f: &var Formatter)` method that writes into a `Formatter` and returns a `Result`, is [deprecated], replace it with `def to_s`.

## 9. Width and precision with your own types

Because `to_s` returns a plain `String`, every padding method from section 4 works on it: `m.to_s.rjust(10)` [implemented].

## 10. Common mistakes

- **Writing `def to_s` with a parameter.** `puts` looks for `to_s` with no arguments that returns a `String`.
- **Expecting `"#{obj}"` to use your `to_s` today.** It does not yet [known issue](../../TODO.md#t-interp-to-s); write `"#{obj.to_s}"`.
- **Expecting `:x` or `:b` for hex / binary.** Those flags don't exist. Call `n.to_s(16)` or `n.to_s(2)` [implemented] and interpolate the result instead.
- **Width on `:?`.** The debug path ignores width and alignment, so `"#{value:>10?}"` won't pad. Render with `to_s` and pad with `rjust`.

> **Try it:** add a `to_s` method to the `Point` class from earlier and make it render as `"(1, 2)"`. Compare `puts pt` before and after.

---

## Recap

- `"#{expr}"` interpolates any expression as a string [implemented].
- `p x` prints a debug view of built-in values [implemented]; for your own classes it is [planned](../../TODO.md#t-d9). `"#{expr:?}"` still works and is [deprecated].
- Pad with `rjust`, `ljust` and `center`, with an optional fill string [implemented]. The interpolation specs for width (`:>5`), alignment (`<`, `>`, `^`), fill (`*<5`) and precision (`.2`, `.5`) are [deprecated] until `format` lands [planned](../../TODO.md#t-d9).
- For your own types, define `def to_s -> String` [implemented].

**Next:** [Chapter 18 — Standard Library Tour](18-stdlib-tour.md) — a "what's in the box" walkthrough of the modules you'll reach for most often.
