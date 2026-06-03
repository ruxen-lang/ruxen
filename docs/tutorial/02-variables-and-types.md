# Variables and Types

This chapter shows how to give names to values and what kinds of values Ruxen knows about. A **value** is a piece of data your program manipulates — a number, a piece of text, a true/false flag. A **type** is the category that value belongs to — `Int`, `String`, `Bool`. Ruxen figures out the type from context most of the time, so you write less and read more.

## A first runnable example

```ruxen
def main
  let name = "Ruxen"
  let year = 2026
  puts "Hello from #{name} in #{year}!"
end
```

Save as `intro.rx` and run:

```bash
ruxen compile intro.rx
./intro
```

Output:

```
Hello from Ruxen in 2026!
```

You just used two bindings (`name`, `year`), two different types (text and integer), and **string interpolation** — the `#{...}` syntax that drops a value into a string.

## `let` vs `var`

Ruxen has two ways to name a value:

- `let` makes an **immutable** binding — once assigned, it cannot be changed.
- `var` makes a **mutable** binding — you can reassign it later.

```ruxen
def main
  let name = "Alaric"
  var counter = 0

  counter = counter + 1     # OK — counter is var
  counter += 1              # same thing, shorter

  puts "#{name} has counter #{counter}"
  # name = "Voss"           # would not compile — name is immutable
end
```

Prefer `let` whenever you can. Immutable bindings are easier to reason about because the value can't change underneath you.

## Type inference

Ruxen looks at the right-hand side and figures out the type for you. You rarely need to write the type yourself:

```ruxen
def main
  let x = 42                 # Int
  let y = 3.14               # Float
  let name = "Ruxen"         # &str (a borrowed piece of text)
  let flag = true            # Bool
  let ch = ?R                # Char (a single Unicode character)

  puts "#{x} #{y} #{name} #{flag} #{ch}"
end
```

You can spell out the type when you want to be explicit or when the default would be wrong:

```ruxen
let temperature: Float = 42         # 42 is interpreted as 42.0
let bytes: Array[UInt8] = Array.new # empty array of unsigned bytes
```

## Primitive types

These are the built-in scalar types — values that fit in a register and copy cheaply:

| Type | Size | Description |
|------|------|-------------|
| `Int` | 64-bit | Default signed integer |
| `Int8`, `Int16`, `Int32`, `Int64` | 8 to 64 bit | Signed with explicit width |
| `UInt` | 64-bit | Default unsigned integer |
| `UInt8`, `UInt16`, `UInt32`, `UInt64` | 8 to 64 bit | Unsigned with explicit width |
| `ISize`, `USize` | pointer-width | Signed/unsigned values the size of a memory address |
| `Float` | 64-bit | Default IEEE 754 double-precision float |
| `Float32`, `Float64` | 32 or 64 bit | Floats with explicit width |
| `Bool` | 1 byte | `true` or `false` |
| `Char` | 4 bytes | A single Unicode scalar value |
| `nil` | 0 bytes | The empty value — used for "nothing to return" |

`nil` is unusual: it's both the type name (`def f -> nil` — "this function returns nothing") and the value (`Ok(nil)` — "the Ok case carries no payload"). You'll see it a lot.

## Numeric literals

```ruxen
42              # Int
42u             # UInt
42i32           # Int32
42u8            # UInt8

1_000_000       # underscores are ignored, used for readability
0xFF            # hexadecimal (255)
0b1010          # binary (10)
0o777           # octal (511)

3.14            # Float
3.14f32         # Float32
1.0e10          # scientific notation
```

## Strings

Ruxen has two string flavours:

| Type | Owned? | Growable? | Use it when |
|------|--------|-----------|-------------|
| `String` | Yes — owns its memory | Yes | You need to keep the text around or change it |
| `&str` | No — borrows from someone else | No | You just need to read existing text |

The `&` in `&str` means **borrow** — you're reading data that someone else owns. Borrowing is covered in detail in [Chapter 4](04-ownership-and-borrowing.md); for now, just know that string literals like `"hello"` are `&str` (borrowed from the program's read-only data) and methods like `String.from(...)` build an owned `String`.

```ruxen
def main
  let greeting = "hello"                  # &str — a string literal
  let owned = String.from("hello")        # String — heap-allocated, owned

  puts greeting
  puts owned
end
```

### String interpolation

Put any expression inside `#{...}` to drop its value into a string:

```ruxen
def main
  let name = "Ruxen"
  let age = 1
  puts "#{name} is #{age} year old"
end
```

Output:

```
Ruxen is 1 year old
```

Chapter 17 goes deeper on formatting; this much is enough for now.

### Raw and multiline strings

Single quotes make a **raw** string — backslashes stay literal and there's
no `#{}` interpolation. Double quotes interpolate and process escapes. (A
raw string can hold `"` freely, but not a `'`.)

```ruxen
let raw = 'no\escape\here'            # backslashes stay literal, no interpolation
let raw2 = 'can have "quotes" inside' # double quotes are fine in a raw string

let multi = """
  This is a
  multiline string
"""
```

## Tuples

A **tuple** is a fixed-size grouping of values, possibly of different types. Use one when you want to return two things from a function or pass a few related values together without defining a whole class:

```ruxen
def divmod(a: Int, b: Int) -> (Int, Int)
  (a / b, a % b)
end

def main
  let (q, r) = divmod(17, 5)
  puts "q=#{q}"
  puts "r=#{r}"
end
```

Output:

```
q=3
r=2
```

The `let (q, r) = ...` line is **destructuring** — unpacking a tuple into named pieces.

## Arrays

A growable, heap-allocated sequence. The literal `[1, 2, 3]` builds an `Array[Int]`:

```ruxen
def main
  var v: Array[Int] = Array.new
  v.push(1)
  v.push(2)
  v.push(3)
  puts "#{v.size}"     # 3
end
```

The fixed-size form `[Int; 3]` exists for stack-allocated arrays with a known length — most code uses the growable `Array[T]` instead.

## Type aliases

Give a long type a short name:

```ruxen
type UserId = Int
type Callback = Fn(Int) -> Bool
```

A type alias is purely a naming convenience — `UserId` and `Int` are the same type to the compiler. For a *distinct* type that wraps `Int`, use a newtype (see [Chapter 6](06-classes-and-structs.md)).

## Constants

Module-level `let` bindings serve as program-wide constants. By convention, name them in `SCREAMING_SNAKE_CASE`:

```ruxen
let MAX_RETRIES = 3
let DEFAULT_PORT: UInt16 = 8080

def main
  puts "retries=#{MAX_RETRIES} port=#{DEFAULT_PORT}"
end
```

There is no separate `const` keyword for value bindings — a top-level `let` *is* the constant form. A constant whose initializer is a simple compile-time expression can also be used in type positions like array sizes.

## Common mistakes

**Trying to reassign a `let`.**

```ruxen
let count = 0
count = count + 1     # ERROR: cannot assign to immutable `count`
```

Change `let` to `var`.

**Mismatched numeric types.**

```ruxen
let x: Int = 1
let y: Float = 2.0
let z = x + y         # ERROR: Int and Float don't auto-convert
```

Convert explicitly: `let z = (x as Float) + y`.

**Treating `&str` and `String` as interchangeable.** They aren't, but Ruxen will often convert for you at call boundaries. When the compiler complains, `&owned` borrows a `String` as `&String`, and `owned.as_str` gets you an `&str`. We'll come back to this once you've met borrowing properly in Chapter 4.

## Try it

Change `intro.rx` so that `year` is a `Float` (`let year: Float = 2026`). Recompile. The interpolation still works — Ruxen knows how to render any printable value. Now try printing a tuple: `puts "#{(1, 2)}"` — what does it look like?

## Recap

- `let` is immutable, `var` is mutable. Prefer `let`.
- Types are usually inferred. Add `: Type` when you need to be specific.
- Primitives include `Int`, `Float`, `Bool`, `Char`, and the unit type `nil`.
- `String` is owned and growable; `&str` is borrowed and read-only.
- `#{expr}` interpolates a value into a string.
- Tuples group a few values without ceremony; arrays grow.

**Next:** [Functions](03-functions.md) — defining your own building blocks.
