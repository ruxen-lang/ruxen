# Variables and Types

This chapter shows how to give names to values and what kinds of values Ruxen knows about. A **value** is a piece of data your program manipulates — a number, a piece of text, a true/false flag. A **type** is the category that value belongs to — `Int`, `String`, `Bool`. Ruxen figures out the type from context most of the time, so you write less and read more.

## A first runnable example

Save this as `intro.rx` [implemented]:

```ruby
name = "Ruxen"
year = 2026
puts "Hello from #{name} in #{year}!"
```

Compile and run:

```bash
ruxen compile intro.rx
./intro
```

Output:

```text
Hello from Ruxen in 2026!
```

You just used two local variables (`name`, `year`), two different types (text and integer), and **string interpolation** — the `#{...}` syntax that drops a value into a string.

## Local variables

As in Ruby, assigning to a new name creates a local variable, and you can
reassign it later [implemented]:

```ruby
name = "Alaric"
counter = 0

counter = counter + 1     # reassign
counter += 1              # same thing, shorter

puts "#{name} has counter #{counter}"
```

A local's type is fixed by its first assignment. Assigning a value of
another type later is a type error, not a new variable (see "Common
mistakes" below).

Older code declares locals with `let` (immutable) or `var` (mutable). Both
keywords are [deprecated]: replace them with plain `x = 1`. They still
compile inside a `def`, but a top-level `let` no longer parses.

## Type inference

Ruxen looks at the right-hand side and figures out the type for you. You rarely need to write the type yourself [implemented]:

```ruby
x = 42                 # Int
y = 3.14               # Float
name = "Ruxen"         # String (owned text)
flag = true            # Bool
ch = ?R                # Char (a single Unicode character)

puts "#{x} #{y} #{name} #{flag} #{ch}"
```

When you want to be explicit, or the default would be wrong, the Ruby form
puts a type annotation on the local [planned](../../TODO.md#t-typed-local):

```ruby
temperature: Float = 42      # 42 is interpreted as 42.0
bytes: Array[UInt8] = []     # empty array of unsigned bytes
```

Works today, until the typed local lands: a typed `let` inside a `def` [deprecated]:

```ruby
def setup
  let temperature: Float = 42      # 42 is interpreted as 42.0
  let bytes: Array[UInt8] = []     # empty array of unsigned bytes
  puts temperature
end

setup()
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

Use `Int` and `Float` in everyday code. The sized types exist for FFI and
compact storage.

`nil` is unusual: it's both the type name (`def f -> nil` — "this method returns nothing") and the value (`nil` — "nothing here"). You'll see it a lot.

## Numeric literals

[implemented]:

```ruby
a = 42              # Int
b = 42u             # UInt
c = 42i32           # Int32
d = 42u8            # UInt8

e = 1_000_000       # underscores are ignored, used for readability
f = 0xFF            # hexadecimal (255)
g = 0b1010          # binary (10)
h = 0o777           # octal (511)

i = 3.14            # Float
j = 3.14f32         # Float32
k = 1.0e10          # scientific notation
```

The suffixes (`u8`, `f32`) are an open question: they may be replaced by
conversion methods such as `200.to_u8` [planned](../../TODO.md#t-oq).

## Strings

Ruxen has one string type and one way to borrow it:

| Type | Owned? | Growable? | Use it when |
|------|--------|-----------|-------------|
| `String` | Yes — owns its memory | Yes | You need to keep the text around or change it |
| `&String` | No — borrows from someone else | No | You just need to read existing text |

The `&` in `&String` means **borrow** — you're reading data that someone else owns. Borrowing is covered in detail in [Chapter 4](04-ownership-and-borrowing.md); for now, just know that a bare string literal like `"hello"` is an owned `String`, and it satisfies a `&String` parameter wherever a method only needs to read it. There is no separate `str` / `&str` type. (The full string-literal model — `""` owned vs `&""` borrowed, and when you'd reach for `.clone` to copy a borrow — is in [Chapter 29](29-strings-bytes-numbers.md).)

[implemented]:

```ruby
greeting = "hello"                  # String — heap-allocated, owned
view = &greeting                    # &String — a read-only borrow

puts greeting
puts view
```

### String interpolation

Put any expression inside `#{...}` to drop its value into a string [implemented]:

```ruby
name = "Ruxen"
age = 1
puts "#{name} is #{age} year old"
```

Output:

```text
Ruxen is 1 year old
```

Chapter 17 goes deeper on formatting; this much is enough for now.

### Raw and multiline strings

Single quotes make a **raw** string — backslashes stay literal and there's
no `#{}` interpolation. Double quotes interpolate and process escapes. (A
raw string can hold `"` freely, but not a `'`.) Triple quotes make a
multi-line string [implemented]:

```ruby
raw = 'no\escape\here'            # backslashes stay literal, no interpolation
raw2 = 'can have "quotes" inside' # double quotes are fine in a raw string

multi = """
  This is a
  multiline string
"""
puts multi
```

Ruby's heredoc `<<~EOS ... EOS` is the multi-line form to come
[planned](../../TODO.md#t-heredoc); until then, use `"""`.

## Returning two values

To return two things from a method, return an array. The Ruby form
destructures the result straight into two locals, with the RBS tuple type
`[Int, Int]` as the return type [planned](../../TODO.md#t-parallel-assign)
(the tuple type is [planned](../../TODO.md#t-tuple-type) too):

```ruby
def divmod(a: Int, b: Int) -> [Int, Int]
  [a / b, a % b]
end

q, r = divmod(17, 5)
puts "q=#{q}"
puts "r=#{r}"
```

Works today: return an `Array[Int]` and index it [implemented]:

```ruby
def divmod(a: Int, b: Int) -> Array[Int]
  [a / b, a % b]
end

qr = divmod(17, 5)
puts "q=#{qr[0]}"
puts "r=#{qr[1]}"
```

Output:

```text
q=3
r=2
```

Older code uses Rust-style tuples, `(Int, Int)` with `let (q, r) = ...`
inside a `def`. Tuples are [deprecated]: replace them with arrays, the
`[Int, Int]` tuple type and `q, r = pair`.

## Arrays

A growable, heap-allocated sequence. The literal `[1, 2, 3]` builds an `Array[Int]` [implemented]:

```ruby
v = [1, 2]
v.push(3)
puts v.size     # 3
```

An empty array needs its element type. The Ruby form is the typed local
`v: Array[Int] = []` [planned](../../TODO.md#t-typed-local); today write
`let v: Array[Int] = []` inside a `def` ([deprecated], as above).

The fixed-size form `[Int; 3]` exists for stack-allocated arrays with a known length — most code uses the growable `Array[T]` instead. Whether it stays (perhaps as `StaticArray[T, N]`) is an open question [planned](../../TODO.md#t-oq).

## Type aliases

Give a long type a short name. The Ruby form is a constant-style assignment
[planned](../../TODO.md#t-type-alias):

```ruby
UserId = Int
Callback = Fn[(Int) -> Bool]
```

Works today: the `type` keyword, [deprecated] until `UserId = Int` lands:

```ruby
type UserId = Int
type Callback = Fn[(Int) -> Bool]

def same(id: UserId) -> UserId = id
puts same(3)
```

A type alias is purely a naming convenience — `UserId` and `Int` are the same type to the compiler. For a *distinct* type that wraps `Int`, use a one-field value type (see [Chapter 6](06-classes-and-structs.md)).

## Constants

Name program-wide constants in `SCREAMING_SNAKE_CASE`. The Ruby form is a
plain assignment at the top level or in a class or module body
[planned](../../TODO.md#t-constants):

```ruby
MAX_RETRIES = 3
DEFAULT_PORT = 8080

puts "retries=#{MAX_RETRIES} port=#{DEFAULT_PORT}"
```

Works today, until `MAX_RETRIES = 3` lands: the `const` keyword [deprecated]:

```ruby
const MAX_RETRIES = 3
const DEFAULT_PORT: UInt16 = 8080u16

def report
  puts "retries=#{MAX_RETRIES} port=#{DEFAULT_PORT}"
end

report()
```

A top-level `let` is not a constant: it does not parse. A constant whose initializer is a simple compile-time expression can also be used in type positions like array sizes.

## Common mistakes

**Changing a local's type.** Rejected today [implemented]:

```ruby
count = 0
count = "zero"     # ERROR: type mismatch: expected `Int`, found `String`
```

Use a new name for the new value.

**Mismatched numeric types.** Rejected today with E0707 [implemented]:

```ruby
x = 1
y = 2.0
z = x + y         # ERROR: Int and Float don't auto-convert
```

Convert explicitly [implemented]:

```ruby
x = 1
y = 2.0
z = x.to_f + y
puts z
```

The Rust cast `x as Float` is [deprecated]: use `to_f` and `to_i`.

**Treating `&String` and `String` as interchangeable.** They aren't, but Ruxen will often convert for you at call boundaries. When the compiler complains, `&owned` borrows a `String` as `&String`, and `borrowed.clone` copies a borrow into a fresh owned `String`. We'll come back to this once you've met borrowing properly in Chapter 4.

## Try it

Change `intro.rx` so that `year` is a `Float` (`year = 2026.0`). Recompile. The interpolation still works — Ruxen knows how to render any printable value (a whole `Float` prints without its `.0` today, a [known issue](../../TODO.md#t-float-to-s)). Now try printing an array: `puts "#{[1, 2]}"` — what does it look like?

## Recap

- `x = 1` creates a local; reassign it freely, but keep its type. `let` and `var` are deprecated.
- Types are usually inferred. A typed local `x: Type = ...` is planned; today use `let x: Type = ...` inside a `def`.
- Primitives include `Int`, `Float`, `Bool`, `Char`, and `nil`.
- `String` is owned and growable; `&String` is borrowed and read-only.
- `#{expr}` interpolates a value into a string.
- Return two values as an array; tuples are deprecated.

**Next:** [Methods](03-functions.md) — defining your own building blocks.
