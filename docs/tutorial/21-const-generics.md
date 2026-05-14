# Const Generics

> **Status:** Stages 1–8 shipped (S8.S1 through S8.S4 + diagnostics
> E0702/E0703/E0704/E0705).  Stage 9 (`where`-clause const
> predicates) and the S7 per-instantiation binding-threading
> follow-up remain.
>
> **See also:** [Spec — const generics](../specs/types/const-generics.spec.md)
> for the full nine-stage roadmap and behaviour catalogue.

Const generics let types and functions be parameterised by a
compile-time **value** — typically an integer that determines an
array size, a fixed-capacity buffer length, or a build-time toggle.

```riven
struct Vector[T, const N: USize]
  data: [T; N]
end

class Matrix[T, const M: USize, const N: USize]
  rows: USize
end

def rotate[const K: USize](x: Int) -> Int
  x
end
```

Riven's design follows Rust's `min_const_generics` plus simple
arithmetic on const expressions.  No general compile-time-function
evaluation; no recursion or branching at the const level.

---

## Declaring const parameters

The parser accepts `const NAME: Type` anywhere a generic parameter
can go:

```riven
struct Vector[T, const N: USize] end             # struct
class SmallVec[T, const N: USize] end            # class
def rotate[const K: USize](x: Int) end           # function
mixin FixedBuffer[const CAP: USize] end          # mixin
extension SmallVec[T, const N: USize] end        # extension
```

Multiple const params and mixed type/const ordering both work:

```riven
struct Matrix[T, const M: USize, const N: USize] end
struct Buffer[const CAP: USize, T] end       # const-first is legal
```

The convention is **types first, consts after** (matches Rust).  The
formatter enforces this; the parser is permissive.

The declared type must be an integer family (`Int`, `Int8`/`Int16`/
`Int32`/`Int64`, `UInt8`/`UInt16`/`UInt32`/`UInt64`, `USize`,
`ISize`) or `Bool`.  Anything else surfaces as
[**E0705**](../errors/E0705.md):

```riven
struct Buf[T, const N: Float] end    # error[E0705]
struct Bag[T, const N: String] end   # error[E0705]
```

---

## Passing const arguments at use sites

### Bare literals

Integer literals are the simplest form:

```riven
struct Holder
  v: Vector[Int, 4]
  m: Matrix[Float, 3, 4]
  x: Foo[Int, 8, Bar]    # type / const / type ordering
end
```

### Arithmetic

`+ - * /` and parens work in both **array-size position** and
**const-arg position**:

```riven
struct Buf
  data: [Int; 2 + 3]            # array size with arithmetic
  pad:  [Int; (4 + 4) * 2]      # parens for grouping
end

class Vector[T, const N: USize]
  data: [T; N + 1]              # in-scope const param + literal
end

def take_one(v: Vector[Int, 2 + 3]) end   # arithmetic at use site
```

Two different source forms that denote the same compile-time integer
produce **the same type** thanks to a normal-form rewriter:

```riven
def need_four(v: Vector[Int, 4]) end

def main
  need_four(Vector.new(0) : Vector[Int, 2 + 2])   # OK — 2 + 2 folds to 4
  need_four(Vector.new(0) : Vector[Int, 4 * 1])   # OK — 4 * 1 folds to 4
  need_four(Vector.new(0) : Vector[Int, 4 + 0])   # OK — N + 0 folds to N (and 4)
end
```

The rewriter applies algebraic identities (`x + 0 = x`,
`x - 0 = x`, `x * 1 = x`, `x * 0 = 0`, `x / 1 = x`) and constant-
folds pure `Lit ⊙ Lit` arithmetic.  It deliberately does NOT
distribute (`N * (M + 1)` vs. `N*M + N`), reorder commutatively, or
reassociate — those are intentional v1 limits (spec §B8).

### What's NOT a valid const expression

Anything outside `Lit`, `Param`, and `+ - * /` arithmetic surfaces
as [**E0702**](../errors/E0702.md):

```riven
struct Bad
  data: [Int; 5 % 2]      # error[E0702]: `%` not in v1 const language
end

struct Worse
  data: [Int; 3 < 4]      # error[E0702]: comparisons not allowed
end

def count -> Int; 4; end
struct Tricky
  data: [Int; count()]    # error[E0702]: function calls not allowed
end
```

There's also no const-arg **inference** (spec OQ-3) — write the
const argument explicitly at every use site.

### Overflow and division by zero

Pure-literal const arithmetic that overflows `u64` or divides by
zero surfaces as [**E0703**](../errors/E0703.md):

```riven
struct Boom
  data: [Int; 9223372036854775807 * 4]    # error[E0703]: overflows
end

struct DivZero
  data: [Int; 10 / 0]                     # error[E0703]: divides by zero
end
```

Param-bearing expressions (`N + 1`) defer this check — overflow
status depends on the eventual instantiation, and per-instantiation
checking lands with the S7 binding-threading follow-up.

### Kind mismatches

Passing a const where a type is expected (or vice versa) surfaces as
[**E0704**](../errors/E0704.md):

```riven
class OnlyType[T] end

let _x: OnlyType[4] = ...    # error[E0704]: kind mismatch (const where type expected)
```

---

## Distinct const args produce distinct types

After Stage 5, two instantiations of the same generic that differ
only in their const args are **distinct types**:

```riven
class SmallVec[T, const N: USize] end

let a: SmallVec[Int, 3] = ...
let b: SmallVec[Int, 4] = a   # type error: cannot assign 3-vec to 4-vec slot
```

This is the soundness gap S5 closes — before that fix, both shapes
folded to `SmallVec[Int]` and silently swapped.

---

## Diagnostic summary

| Code  | When it fires                                                            | Tutorial section |
|-------|--------------------------------------------------------------------------|------------------|
| [E0702](../errors/E0702.md) | Expression isn't a valid v1 const expression               | "What's NOT a valid const expression" |
| [E0703](../errors/E0703.md) | Pure-literal overflow or `_ / 0`                            | "Overflow and division by zero" |
| [E0704](../errors/E0704.md) | Kind mismatch — const in type slot, or vice versa           | "Kind mismatches" |
| [E0705](../errors/E0705.md) | Const-param type isn't integer or `Bool`                    | "Declaring const parameters" |
| E0701 | (reserved) — wrong const-arg type once it splits from E0704              | n/a |

---

## What's still pending

Two follow-ups remain for the v1 const-generics story:

- **S7 binding-threading** — `codegen/layout.rs::layout_of` is
  called with an empty `HashMap<String, u64>` today, so
  `struct Buf[T, const N: USize]` with field `data: [T; N]` produces
  a 0-byte field at the layout pass.  The monomorphization wrapper
  needs to thread per-instantiation bindings through.  Once this
  lands, per-instantiation overflow / div-zero detection (the
  deferred half of E0703) flips on automatically.
- **S9 `where`-clause const predicates** — `where N > 0`,
  `where N == M`, `where N + M == 8` evaluated at monomorphization;
  failing predicate emits `E-CONST-WHERE-FALSE` (a new E07xx slot
  TBD).

Everything else listed in spec §B1–§B8 is wired through.

---

## Out of scope (matches Rust `min_const_generics`)

These are explicit non-goals — they won't ship even in v2.next:

- **Arbitrary compile-time-evaluated functions** (Zig's `comptime` model is a separate feature).
- **Floating-point const generics** (`NaN != NaN` breaks type equality).
- **`String` / `&str` const generics**.
- **Const-generic type parameters** (`def foo[const T: Type]`).
- **Const-generic specialization**.
- **Defaults for const generics** (`const N: USize = 4`).
- **Inference of const arguments** (Rust says no in `min_const_generics`).

---

## How to read this chapter

If something here doesn't work the way the text claims, the spec is
authoritative: [`docs/specs/types/const-generics.spec.md`](../specs/types/const-generics.spec.md).
The pin tests in `crates/riven-core/tests/const_generics.rs` are
the executable contract — every behaviour above corresponds to a
named test.

---

**Next:** [Chapter 22 — Concurrency Primitives](22-concurrency-primitives.md)
covers the next big surface (Thread, Mutex, SharedSync).
