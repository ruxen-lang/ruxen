# Variables and Types

## Variable Bindings

Riven has two binding keywords. `let` is immutable; `var` is mutable.

```riven
let name = "Alaric"
let age = 30
let pi = 3.14

name = "Voss"   # COMPILE ERROR: `name` is immutable
```

Use `var` to make a variable mutable:

```riven
var counter = 0
counter = counter + 1
counter += 1              # compound assignment
```

## Type Inference

Riven infers types from the right-hand side. You rarely need to write type annotations:

```riven
let x = 42                # Int
let y = 3.14              # Float
let name = "Riven"        # &str (borrowed string)
let flag = true           # Bool
let ch = 'R'              # Char
```

You can add explicit type annotations when needed:

```riven
let x: Float = 42         # 42 interpreted as Float
var bytes: Array[UInt8] = Array.new
```

## Primitive Types

| Type | Size | Description |
|------|------|-------------|
| `Int` | 64-bit | Default signed integer |
| `Int8`, `Int16`, `Int32`, `Int64` | 8-64 bit | Explicit-width signed |
| `UInt` | 64-bit | Default unsigned integer |
| `UInt8`, `UInt16`, `UInt32`, `UInt64` | 8-64 bit | Explicit-width unsigned |
| `ISize`, `USize` | pointer-width | Signed/unsigned pointer-sized |
| `Float` | 64-bit | Default IEEE 754 double |
| `Float32`, `Float64` | 32/64 bit | Explicit-width floats |
| `Bool` | 1 byte | `true` or `false` |
| `Char` | 4 bytes | Unicode scalar value |
| `()` | 0 bytes | Unit type |

## Numeric Literals

```riven
42              # Int
42u             # UInt
42i32           # Int32
42u8            # UInt8

1_000_000       # underscores for readability
0xFF            # hexadecimal
0b1010          # binary
0o777           # octal

3.14            # Float
3.14f32         # Float32
1.0e10          # scientific notation
```

## Strings

Riven has two string types:

| Type | Ownership | Growable | When |
|------|-----------|----------|------|
| `String` | Owned, heap-allocated | Yes | You need to own or modify the string |
| `&str` | Borrowed slice | No | Read-only access to string data |

```riven
let greeting = "hello"               # &str (static, borrowed)
let owned = String.new("hello")      # String (owned)
let interpolated = "hi #{name}"      # String (interpolation allocates)
```

### String Interpolation

Use `#{}` inside double-quoted strings:

```riven
let name = "Riven"
let age = 1
puts "#{name} is #{age} year old"
```

### Raw and Multiline Strings

```riven
let raw = r"no\escape\here"          # raw string
let raw2 = r#"can have "quotes""#    # raw with delimiters

let multi = """
  This is a
  multiline string
"""
```

## Tuples

Fixed-size, heterogeneous collections:

```riven
let point = (3, 4)                   # (Int, Int)
let record = ("Alice", 30, true)     # (String, Int, Bool)
let (x, y) = point                   # destructuring
```

## Fixed Arrays and Growable Arrays

```riven
# Fixed-size arrays — stack-allocated
let nums: [Int; 3] = [1, 2, 3]

# Growable arrays — heap-allocated. The bare `[…]` literal builds an
# `Array[T]`; the fixed-size form is only chosen when the binding has
# an `[T; N]` annotation.
var v = [1, 2, 3]
v.push(4)
```

## Type Aliases

```riven
type UserId = Int
type Callback = Fn(Int) -> Bool
```

## Constants

```riven
const MAX_RETRIES = 3
const DEFAULT_PORT = 8080
```
