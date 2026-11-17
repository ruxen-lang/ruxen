# Strings, Bytes, and Numbers

These three primitive shapes — text, raw bytes, and numbers — show up in almost every Ruxen program. Strings come with one owned type and one borrow, bytes live in `Array[UInt8]`, and numbers live on a small ladder of sized integer and float types. This chapter is a working tour: build a string, slice it up, convert it to and from bytes, and walk through Ruxen's numeric types and conversions.

---

## 1. A first example

Save as `strings.rx`. Example [implemented]:

```ruby
name = "Ruxen"
greeting = "hello, #{name}!"
puts greeting
puts "bytes = #{greeting.size}"
```

Run:

```sh
ruxen run strings.rx
```

Output:

```text
hello, Ruxen!
bytes = 13
```

Notice three things: a bare string literal (`"Ruxen"`) is already an owned `String` here (no wrapper needed), `"#{...}"` interpolation builds a new owned string, and `.size` returns the byte count.

---

# Part 1: Strings

## 2. `String` and `&String`

Ruxen has exactly **one** string type pair — pick by ownership:

| Type      | Owns memory? | Growable? | Cheap to copy? | When                               |
|-----------|--------------|-----------|----------------|------------------------------------|
| `String`  | Yes (heap)   | Yes       | No (move)      | You own it, build it, or mutate it |
| `&String` | No (borrow)  | No        | Yes (a ref)    | You're just reading                |

Example [implemented]:

```ruby
owned = "hello"                           # owned, heap-allocated

def shout(s: &String) -> String           # borrows — the caller keeps ownership
  "#{s}!"
end
puts shout("hi")                          # bare literal satisfies the &String param
```

Interpolation (`"hi #{name}"`) always allocates a `String` — the result is owned, not a slice into the source code.

> **The string model — one owned type, one borrow. There is no `str`/`&str`.**
>
> - **`"text"`** is an **owned `String`** (heap-allocated, dropped at end of
>   scope) — not a borrow. Bind it (`s = "text"`) and you own a `String`; at
>   a *call site* it also satisfies a `&String` parameter (the call site borrows
>   it), so you write `"text"` everywhere and the position decides whether it's
>   the owned value or a borrow of it.
> - **`&some_string`** is the **borrow**: `&String` — a reference to a `String`
>   you (or someone) owns. This is the *only* string borrow type.
> - **`str` / `&str` is not a type** [retired]. Writing `str` in an annotation
>   is an error (E0730) with a hint pointing at `String` / `&String`.
>   `&"text"` is a `&String` borrow of an owned literal — prefer the bare
>   `"text"`.
> - **`b.clone`** copies a **runtime `String` / `&String` borrow** `b` into a
>   fresh owned `String` (when you need to keep it past the borrow). It is
>   **never** needed on a literal, because `"text"` is already an owned `String`.

## 3. Building a `String`

Common starting points. Example [implemented]:

```ruby
a = String.new                     # empty, no allocation
c = "hello"                        # a bare literal is already an owned String
d = "hi #{c}"                      # from interpolation (owned)
e = "abc" * 3                      # "abcabcabc"
```

`"abc".repeat(3)` is the [deprecated] spelling of `"abc" * 3`. `String.with_capacity(64)`, which reserves room for 64 bytes up front, is [deprecated] too; Ruby has no capacity argument, so it will be replaced by `String.new` (capacity is an implementation detail).

## 4. Mutating a `String`

Ruby appends with `s << "world"`; on a `String` that is [implemented] but does not append today [known issue](../../TODO.md#t-string-shovel). Until it is fixed, `push_str` does the job. Works today [deprecated] (`push_str` and `with_capacity`, replaced by `<<` and `String.new`):

```ruby
s = String.with_capacity(32)
s.push_str("hello, ")
s.push_str("world")
puts s                                  # hello, world
```

`push_str` takes a `&String` (borrowed). A bare string literal satisfies that borrow at the call site — no leading `&` needed. To build a new string from two, `"a" + "b"` [implemented] also works.

## 5. Inspecting

Example [implemented]:

```ruby
s = "hello world"

puts s.size                  # byte length
puts s.bytesize              # byte length, Ruby's name
puts s.empty?                # Bool
puts s.include?("world")     # Bool
puts s.start_with?("hello")  # Bool
puts s.end_with?("world")    # Bool
```

`size` and `length` return **bytes** today, while Ruby counts characters there [known issue](../../TODO.md#t-string-size-chars). For a Unicode-aware count, use `.chars.size` [implemented]. The Rust names `len`, `is_empty`, `starts_with` and `ends_with` still work and are [deprecated], replaced by the names above.

## 6. Slicing, splitting, and lines

Example [implemented]:

```ruby
csv = "alpha,beta,gamma"
for piece in csv.split(",")
  puts piece                # alpha, then beta, then gamma
end

block = "line one\nline two\nline three"
for line in block.lines
  puts line
end

trimmed = "   hello   ".strip     # "hello"
```

`split(sep)` and `lines` both return an `Array[String]`. You can `for`-loop over them directly, or index into the array if you need a specific element.

## 7. Case, strip, replace

Example [implemented]:

```ruby
raw = "  Hello World  "

puts raw.strip                 # Hello World
puts raw.strip.upcase          # HELLO WORLD
puts raw.strip.downcase        # hello world

s = "hello world hello"
puts s.gsub("hello", "bye")    # bye world bye
```

`strip` returns a fresh owned `String` (it allocates a stripped copy). The Rust names `trim`, `to_upper`, `to_lower` and `replace` still work and are [deprecated], replaced by `strip`, `upcase`, `downcase` and `gsub`.

## 8. Parsing numbers from strings

Ruby's `"42".to_i` [implemented] returns the number (and `0` for text that is not a number). Ruby's strict `Integer("42")`, which raises on bad input, is [planned](../../TODO.md#t-exceptions). Today the strict form is `parse_int`, which returns a `Result`. Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by `Integer(...)` and `rescue`):

```ruby
case "42".parse_int
in Ok(n) then puts "got #{n}"
in Err(_) then puts "not a number"
end
```

## 9. Iterating over characters

Example [implemented]:

```ruby
count = 0
for ch in "abc".chars
  count += 1
end
puts "#{count}"        # 3
```

- `.chars` returns `Char` values (Unicode scalars) [implemented].
- `.bytes` returns `UInt8` values (raw bytes) [implemented].
- `.lines` returns owned `String` values (one per line) [implemented].

> **Try it:** loop over `"café".chars` and print each character. Then compare `"café".chars.size` with `"café".bytesize`: 'é' is one character but two bytes.

---

# Part 2: Bytes

## 10. `Array[UInt8]` is the byte buffer

There's no separate `Bytes` type — Ruxen uses `Array[UInt8]` for raw byte buffers, with the full `Array` API. Example [implemented]:

```ruby
buf = [72, 105]                  # 'H', 'i'
puts String.from_bytes(buf)      # Hi
```

`String.from_bytes` takes the byte values as an `Array[Int]`.

## 11. String <-> byte conversion

Example [implemented]:

```ruby
bs = "hi".into_bytes          # Array[UInt8], consumes the string
copy = bs.clone               # if you need to keep them around
puts bs.size                  # 2
```

- `.into_bytes` is **consuming** — the source string is moved.
- `.bytes` returns a fresh `Array[UInt8]` if you want to inspect without giving up ownership.

---

# Part 3: Numbers

## 12. The numeric ladder

Example [implemented]:

```ruby
a = 42                    # Int   (signed 64-bit, the default)
b = 3.14                  # Float (IEEE 754 double)
c = 42i32                 # explicit-width signed
d = 255u8                 # explicit-width unsigned
```

| Name                              | Width                         |
|-----------------------------------|-------------------------------|
| `Int8`, `Int16`, `Int32`, `Int64` | 8 / 16 / 32 / 64-bit signed   |
| `UInt8`, `UInt16`, `UInt32`, `UInt64` | 8 / 16 / 32 / 64-bit unsigned |
| `ISize` / `USize`                 | pointer-width                 |
| `Float` / `Float64`               | 64-bit                        |
| `Float32`                         | 32-bit                        |

The default `Int` is signed 64-bit; the default `Float` is 64-bit. Reach for the sized forms when you're matching a wire protocol or trying to fit data in a small buffer. Sized types are meant for FFI and storage; whether they stay in everyday code, and whether the literal suffixes stay, is an open question (OQ-8) [planned](../../TODO.md#t-oq).

## 13. Numeric literals

Example [implemented]:

```ruby
h = 0xFF            # hex
b = 0b1010          # binary
o = 0o777           # octal
g = 1_000_000       # underscores for readability

f1 = 1.0e3          # scientific (positive exponent)
f2 = 2.5e-2         # scientific (negative exponent)

n = 255u8           # suffix forces the width
m = 100i32
```

Hex / binary / octal literals are always integer-typed. Floats need a decimal point or an exponent — `1e3` is a float, `1` is an integer.

## 14. Converting between numeric types

Ruxen has no implicit numeric coercion. Ruby's conversion methods `to_i` and `to_f` [implemented] cover `Int` and `Float`:

```ruby
n = 42
f = n.to_f          # 42.0
i = 3.9.to_i        # 3
```

Sized conversions such as `n.to_u8` are [planned](../../TODO.md#t-sized-conv). Until then the `as` cast does them, and it is [deprecated] (replaced by the `to_*` methods). It is also the way to turn a `USize`, which `size` returns today, into an `Int` (`size` returning `Int` is [planned](../../TODO.md#t-size-int)):

```ruby
n = 42
small = n as UInt8          # truncates if needed

count = "abcd".size
signed = count as Int
```

A cast that doesn't fit (e.g. `300 as UInt8`) truncates by masking — it never aborts. If you need a range-checked conversion, write a method that returns an optional value.

## 15. Common mistakes

- **Treating `size` as character count.** `s.size` is bytes today; "café" has 5 bytes but 4 characters. Use `.chars.size` for the Unicode count.
- **`push_str` takes a `&String`.** A bare literal works — `s.push_str("hi")` — because the call site borrows the owned literal; no leading `&` needed.
- **Expecting `<<` to append today.** It does not yet [known issue](../../TODO.md#t-string-shovel); use `push_str`, interpolation `"#{a}#{b}"` or `a + b`.
- **Implicit numeric promotion.** Adding `Int` and `Int32` is a type error — convert one side explicitly.
- **Assuming `into_bytes` is borrowing.** It's consuming; the source string is moved out. Use `.bytes` or `.clone.into_bytes` to keep the original around.

> **Try it:** build a byte array holding the UTF-8 bytes of `"hi"` (don't use `into_bytes` — write the integer values directly), then convert back to a `String` with `String.from_bytes`.

---

## Recap

- **`String`** owns its memory and is mutable; **`&String`** is a cheap borrow (a reference) you can pass around. There is no `str`/`&str` — one owned type, one borrow.
- A bare literal `"text"` is already an owned `String`; build also with `String.new`, interpolation, `+` or `*`. To copy a *runtime* `String` / `&String` borrow `b` into a fresh owned `String`, use `b.clone` — never needed on a literal (a literal is already owned).
- Bytes live in `Array[UInt8]`; convert with `.into_bytes` and `String.from_bytes`.
- Numbers come in sized signed / unsigned integers, two float widths, and pointer-width `USize` / `ISize`.
- `to_i` and `to_f` convert [implemented]; sized conversions use the [deprecated] `as` until `to_u8` and friends land [planned](../../TODO.md#t-sized-conv).

**Next:** [Chapter 30 — Lifetimes and Borrowing in Depth](30-lifetimes-advanced.md).
