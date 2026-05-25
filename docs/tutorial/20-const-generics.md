# Const Generics

What if you wanted a `Vector` whose size was baked into the type — so the compiler could check at compile time that you never asked for the 6th element of a 5-element vector, or that two vectors you're adding together are the same length? That's what **const generics** are for. Where a regular generic parameter (`[T]`) lets the type take any *type*, a const generic (`[const N: USize]`) lets the type take a specific *value* — typically a small integer — that the compiler knows at compile time and can reason about.

This chapter shows how to declare a const generic parameter, pass values for it at the call site, and what limits Ruxen places on what counts as a "value the compiler can know".

---

## 1. A first example

Save as `vec_demo.rx`:

```ruxen
struct Vector[T, const N: USize]
  data: [T; N]
end

def main
  let v: Vector[Int, 3] = Vector { data: [1, 2, 3] }
  puts "len = 3, first = #{v.data[0]}"
end
```

Run it:

```bash
ruxen run vec_demo.rx
```

Output:

```
len = 3, first = 1
```

`Vector[Int, 3]` and `Vector[Int, 4]` are now **different types** as far as the compiler is concerned. Try to mix them up and you get a compile error before the program ever runs.

## 2. Declaring const parameters

Anywhere you can declare a generic type parameter (`[T]`), you can declare a const parameter (`[const NAME: Type]`):

```ruxen
struct Vector[T, const N: USize] end           # struct
class SmallVec[T, const N: USize] end          # class
def rotate[const K: USize](x: Int) end         # function
mixin FixedBuffer[const CAP: USize] end        # mixin
extension SmallVec[T, const N: USize] end      # extension
```

Multiple const params and mixed type / const ordering both work:

```ruxen
struct Matrix[T, const M: USize, const N: USize] end
struct Buffer[const CAP: USize, T] end       # const-first is legal
```

The convention is **types first, consts after** — the formatter enforces this.

### What types can a const parameter be?

Integer types and `Bool`:

```
Int  Int8  Int16  Int32  Int64
UInt8  UInt16  UInt32  UInt64
USize  ISize
Bool
```

A const parameter typed `Float` or `String` is rejected at compile time. Floating-point doesn't work because `NaN != NaN` breaks the compiler's idea of type equality; strings aren't supported in v1.

## 3. Passing const arguments at the call site

### Bare literals

The simplest case — just write the number:

```ruxen
struct Holder
  v: Vector[Int, 4]
  m: Matrix[Float, 3, 4]
end
```

### Simple arithmetic

You can use `+`, `-`, `*`, `/`, and parens in both array-size positions and const-argument positions:

```ruxen
struct Buf
  data: [Int; 2 + 3]            # array size with arithmetic
  pad:  [Int; (4 + 4) * 2]      # parens for grouping
end

class Vector[T, const N: USize]
  data: [T; N + 1]              # in-scope const param plus a literal
end

def take_one(v: Vector[Int, 2 + 3]) end   # arithmetic at use site
```

Two different source expressions that compute to the same value produce **the same type**:

```ruxen
def need_four(v: Vector[Int, 4]) end

def main
  let a: Vector[Int, 2 + 2] = ...
  need_four(a)                # OK — 2 + 2 is 4
  let b: Vector[Int, 4 * 1] = ...
  need_four(b)                # OK — 4 * 1 is 4
end
```

## 4. What's NOT a valid const expression

The const language is deliberately tiny. Anything outside literals, parameters, and basic `+ - * /` arithmetic is rejected:

```ruxen
struct Bad
  data: [Int; 5 % 2]      # error: `%` is not in the const language
end

struct Worse
  data: [Int; 3 < 4]      # error: comparisons not allowed
end

def count -> Int; 4; end
struct Tricky
  data: [Int; count()]    # error: function calls not allowed
end
```

There's also no *inference* of const arguments — every use site has to spell them out explicitly. The compiler will never guess `4` for you.

### Overflow and division by zero

If you write a literal expression that overflows or divides by zero, you get a compile error:

```ruxen
struct Boom
  data: [Int; 9223372036854775807 * 4]    # overflow at compile time
end

struct DivZero
  data: [Int; 10 / 0]                     # division by zero
end
```

Expressions that involve a const *parameter* (`N + 1`) defer the check to the moment of instantiation — if `N` is ever large enough to overflow, the error fires there.

### Kind mismatches

Passing a const where a type is expected (or the other way) is an error:

```ruxen
class OnlyType[T] end

let _x: OnlyType[4] = ...    # error: const where type expected
```

You can call `ruxen explain <code>` on any compiler error to see the long-form explanation with examples.

## 5. Different const values = different types

This is the central point of const generics — and where they earn their keep:

```ruxen
class SmallVec[T, const N: USize] end

let a: SmallVec[Int, 3] = ...
let b: SmallVec[Int, 4] = a   # compile error: cannot assign 3-vec to 4-vec
```

A function that takes `SmallVec[Int, 3]` will refuse anything else — no runtime length checks needed.

## 6. What's intentionally left out

In case you're wondering whether some clever pattern works — these are non-goals for v1:

- **General compile-time function evaluation** (`const fn`).
- **Floating-point const generics** — `NaN` breaks type equality.
- **String const generics** — not in v1.
- **Const-generic specialisation** — one impl for `N == 0`, another for `N > 0`.
- **Defaults for const generics** (`const N: USize = 4`).
- **Inference of const arguments** — always write them explicitly.

## 7. Common mistakes

- **Forgetting `const`.** `struct Vector[T, N: USize]` parses but treats `N` as a *type* parameter named `N` — and `USize` would be the bound, not the type. You'd see a confusing error later. The fix is to write `[T, const N: USize]`.
- **Putting const args before type args at the use site.** `Vector[3, Int]` is parsed but won't match `Vector[T, const N: USize]`. Stick with type-first.
- **Expecting `%` or comparisons to work.** Only `+ - * /` plus parens. If you need anything fancier, lift the value out of the const language and store it as a runtime field instead.

> **Try it:** declare `struct Pair[T, const N: USize]` with a field `data: [T; N * 2]`. Construct a `Pair[Int, 3]` and check `data` is six elements long.

---

## Recap

- Const generics let a type or function take a compile-time *value* (typically an integer) as a parameter.
- Declare with `[const N: USize]`; integer types and `Bool` are allowed.
- Use site uses bare literals or simple `+ - * /` arithmetic on literals / in-scope params.
- Different const values produce **different types** — the whole point.
- No inference, no `%`, no comparisons, no function calls inside const expressions.

**Next:** [Chapter 21 — Concurrency Primitives](21-concurrency-primitives.md).
