# Strings, Bytes, and Numbers

These three primitive shapes — text, raw bytes, and numbers — show up in almost every Ruxen program. Strings come in two flavours (owned and borrowed), bytes live in `Array[UInt8]`, and numbers live on a small ladder of sized integer and float types. This chapter is a working tour: build a string, slice it up, convert it to and from bytes, and walk through Ruxen's numeric types and conversions.

---

## 1. A first example

Save as `strings.rx`:

```ruxen
def main
  let name = "Ruxen"
  let greeting = "hello, #{name}!"
  puts greeting
  puts "bytes = #{greeting.size}"
end
```

Run:

```bash
ruxen run strings.rx
```

Output:

```
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

```ruxen
let owned: String = "hello"               # owned, heap-allocated

def shout(s: &String) -> String           # borrows — the caller keeps ownership
  "#{s}!"
end
puts shout("hi")                           # bare literal satisfies the &String param
```

Interpolation (`"hi #{name}"`) always allocates a `String` — the result is owned, not a slice into the source code.

> **The string model — one owned type, one borrow. There is no `str`/`&str`.**
>
> - **`"text"`** is an **owned `String`** (heap-allocated, dropped at end of
>   scope) — not a borrow. Bind it (`let s = "text"`) and you own a `String`; at
>   a *call site* it also satisfies a `&String` parameter (the call site borrows
>   it), so you write `"text"` everywhere and the position decides whether it's
>   the owned value or a borrow of it.
> - **`&someString`** is the **borrow**: `&String` — a reference to a `String`
>   you (or someone) owns. This is the *only* string borrow type.
> - **`str` / `&str` is not a type.** Writing `str` in an annotation is an error
>   (E0730) with a hint pointing at `String` / `&String`. `&"text"` is a
>   `&String` borrow of an owned literal — prefer the bare `"text"`.
> - **`b.clone`** copies a **runtime `String` / `&String` borrow** `b` into a
>   fresh owned `String` (when you need to keep it past the borrow). It is
>   **never** needed on a literal, because `"text"` is already an owned `String`.

## 3. Building a `String`

Five common starting points:

```ruxen
let a = String.new                     # empty, no allocation
let b = String.with_capacity(64)       # empty, capacity reserved
let c = "hello"                        # a bare literal is already an owned String
let d = "hi #{name}"                   # from interpolation (owned)
let e = "abc".repeat(3)                # "abcabcabc"
```

`with_capacity(n)` is the smart choice when you know you'll push at least N characters — it avoids the regrow tax of pushing into a small buffer over and over.

## 4. Mutating a `String`

```ruxen
var s = String.with_capacity(32)
s.push_str("hello, ")
s.push_str("world")
puts s                                  # hello, world
```

`push_str` takes a `&String` (borrowed). A bare string literal satisfies that borrow at the call site — no leading `&` needed (the bare form is the idiom; `&"..."` is also a `&String` borrow of the literal).

## 5. Inspecting

```ruxen
let s = "hello world"

s.size                       # byte length (Int)
s.empty?                  # Bool
s.include?("world")         # Bool
s.starts_with("hello")      # Bool
s.ends_with("world")        # Bool
```

`size` returns **bytes**, not characters. For a Unicode-aware count, use `.char_count` (or iterate `.chars` and count).

## 6. Slicing, splitting, and lines

```ruxen
let csv = "alpha,beta,gamma"
for piece in csv.split(",")
  puts piece                # alpha, then beta, then gamma
end

let block = "line one\nline two\nline three"
for line in block.lines
  puts line
end

let trimmed = "   hello   ".trim     # "hello"
```

`split(sep)` and `lines` both return an `Array[String]`. You can `for`-loop over them directly, or index into the array if you need a specific element.

## 7. Case, trim, replace

```ruxen
let raw = "  Hello World  "

puts raw.trim                 # Hello World
puts raw.trim.to_upper        # HELLO WORLD
puts raw.trim.to_lower        # hello world

let s = "hello world hello"
puts s.replace("hello", "bye")    # bye world bye
```

`trim` returns a fresh owned `String` (it allocates a trimmed copy).

## 8. Parsing numbers from strings

`parse_int` returns a `Result`:

```ruxen
let s = "42"
match s.parse_int
  Ok(n)  -> puts "got #{n}"
  Err(_) -> puts "not a number"
end
```

Compose with `?` inside `Result`-returning functions for clean parse pipelines.

## 9. Iterating over characters

```ruxen
let s = "abc"
var count = 0
for ch in s.chars
  count += 1
end
puts "#{count}"        # 3
```

- `.chars` yields `Char` values (Unicode scalars).
- `.bytes` yields `UInt8` values (raw bytes).
- `.lines` yields owned `String` values (one per line).

> **Try it:** loop over `"café".chars` and print each character with its `:?` debug form. Notice that 'é' is one character but two bytes.

---

# Part 2: Bytes

## 10. `Array[UInt8]` is the byte buffer

There's no separate `Bytes` type — Ruxen uses `Array[UInt8]` for raw byte buffers. Full `Array` API plus the string round-trip:

```ruxen
var buf: Array[UInt8] = Array.with_capacity(256)
buf.push(72)   # 'H'
buf.push(105)  # 'i'

let s = String.from_utf8(&buf).expect!("valid utf-8")
puts s         # Hi
```

`Array.with_capacity(n)` pre-allocates the backing storage.

## 11. String <-> byte conversion

```ruxen
let s   = "hi"
let bs  = s.into_bytes        # Array[UInt8], consumes s
let dup = bs.clone             # if you need to keep them around

let back = String.from_utf8(&bs).expect!("valid utf-8")
```

- `.into_bytes` is **consuming** — the source string is moved.
- `.bytes` is an iterator if you want to inspect without giving up ownership.
- `String.from_utf8(&bs)` returns `Result[String, Utf8Error]` — non-UTF-8 input is rejected.

---

# Part 3: Numbers

## 12. The numeric ladder

```ruxen
let a = 42                    # Int   (signed 64-bit, the default)
let b = 3.14                  # Float (IEEE 754 double)
let c: Int32  = 42i32         # explicit-width signed
let d: UInt8  = 255u8         # explicit-width unsigned
```

| Name                              | Width                         |
|-----------------------------------|-------------------------------|
| `Int8`, `Int16`, `Int32`, `Int64` | 8 / 16 / 32 / 64-bit signed   |
| `UInt8`, `UInt16`, `UInt32`, `UInt64` | 8 / 16 / 32 / 64-bit unsigned |
| `ISize` / `USize`                 | pointer-width                 |
| `Float` / `Float64`               | 64-bit                        |
| `Float32`                         | 32-bit                        |

The default `Int` is signed 64-bit; the default `Float` is 64-bit. Reach for the sized forms when you're matching a wire protocol or trying to fit data in a small buffer.

## 13. Numeric literals

```ruxen
let h = 0xFF            # hex
let b = 0b1010          # binary
let o = 0o777           # octal
let g = 1_000_000       # underscores for readability

let f1 = 1.0e3          # scientific (positive exponent)
let f2 = 2.5e-2         # scientific (negative exponent)

let n: UInt8  = 255u8   # suffix forces the width
let m: Int32  = 100i32
```

Hex / binary / octal literals are always integer-typed. Floats need a decimal point or an exponent — `1e3` is a float, `1` is an integer.

## 14. Casting with `as`

`as` is the **only** numeric conversion form — Ruxen has no implicit numeric coercion:

```ruxen
let n: Int = 42
let f = n as Float          # 42.0
let small = n as UInt8      # truncates if needed

let count: USize = 16
let signed = count as Int
```

A cast that doesn't fit (e.g. `300 as UInt8`) truncates by masking — it never panics. If you need a range-checked conversion, write a method that returns `Option` or `Result`.

## 15. Common mistakes

- **Treating `size` as character count.** `s.size` is bytes; "café" has 5 bytes but 4 characters. Use `.char_count` for the Unicode count.
- **`push_str` takes a `&String`.** A bare literal works — `s.push_str("hi")` — because the call site borrows the owned literal; no leading `&` needed.
- **Expecting `+` on strings.** Ruxen has no `+` for `String`. Use interpolation: `"#{a}#{b}"`.
- **Implicit numeric promotion.** Adding `Int` and `Int32` is a type error — cast one side explicitly with `as`.
- **Assuming `into_bytes` is borrowing.** It's consuming; the source string is moved out. Use `.bytes` (iterator) or `.clone().into_bytes()` to keep the original around.

> **Try it:** build a byte buffer holding the UTF-8 bytes of `"hi"` (don't use `into_bytes` — push the integer values directly), then convert back to a `String` with `String.from_utf8`.

---

## Recap

- **`String`** owns its memory and is mutable; **`&String`** is a cheap borrow (a reference) you can pass around. There is no `str`/`&str` — one owned type, one borrow.
- A bare literal `"text"` is already an owned `String`; build also with `String.new`, `String.with_capacity`, interpolation, or `.repeat`. To copy a *runtime* `String` / `&String` borrow `b` into a fresh owned `String`, use `b.clone` (or `b.to_string`) — never needed on a literal (a literal is already owned).
- Bytes live in `Array[UInt8]`; convert with `.into_bytes` and `String.from_utf8`.
- Numbers come in sized signed / unsigned integers, two float widths, and pointer-width `USize` / `ISize`.
- `as` is the only numeric conversion form — no implicit coercion.

**Next:** [Chapter 30 — Lifetimes and Borrowing in Depth](30-lifetimes-advanced.md).
