# Const Generics (in flight)

> **Status:** Stage 1 + Stage 2 shipped (commits `b8a371c` and
> `bde3e1f`); Stages 3–9 pending.  This chapter reflects only what's
> shippable today.
>
> **See also:** [Spec — const generics](../specs/types/const-generics.spec.md)
> for the full nine-stage roadmap and behaviour catalogue.

Const generics let types and functions be parameterised by a
compile-time **value** — usually an integer that determines an array
size or a fixed-capacity buffer length.

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

---

## What works today (Stage 1 + Stage 2)

### Stage 1 — declaring const params

The parser accepts `const NAME: Type` anywhere a generic parameter
can go:

```riven
struct Vector[T, const N: USize] end         # struct
class SmallVec[T, const N: USize] end        # class
def rotate[const K: USize](x: Int) end       # fn
trait FixedBuffer[const CAP: USize] end      # trait
impl[T, const N: USize] SmallVec[T, N] end   # impl
```

Multiple const params and mixed type/const ordering both work:

```riven
struct Matrix[T, const M: USize, const N: USize] end
struct Buffer[const CAP: USize, T] end       # const-first is legal
```

The convention is **types first, consts after** (matches Rust).  The
formatter enforces this; the parser is permissive.

### Stage 2 — passing integer literals

At use sites, integer literals can appear in generic-argument
position:

```riven
struct Holder
  v: Vector[Int, 4]
  m: Matrix[Float, 3, 4]
  x: Foo[Int, 8, Bar]       # type / const / type ordering
end
```

Internally the parser emits `TypeExpr::ConstLit(value, span)` for
the literal.  Stage 3 (resolver) will validate that the literal
actually lands against a `const` parameter; today there's no check,
so `Vec[5]` parses but produces a resolver error at typeck time
(it tries to interpret `5` as a type).

---

## What's not yet implemented (Stages 3–9)

| Stage | What it adds                                                | Status   |
|-------|-------------------------------------------------------------|----------|
| S3    | Resolver records `DefKind::ConstParam`; brings `N` into scope inside type/fn bodies | pending |
| S4    | HIR: `Ty::Array(Box<Ty>, ConstExpr)` + `ConstExpr` enum     | pending  |
| S5    | Typeck unification; distinct const args produce distinct types | pending |
| S6    | Monomorphization: one MIR fn per `(type-args, const-args)`  | pending  |
| S7    | Codegen layout: arrays evaluate `ConstExpr`; `alloca` honours it | pending |
| S8    | Arithmetic in const exprs (`+ - * /`), normal-form rewriter | pending  |
| S9    | `where` clause const predicates (`where N > 0`)             | pending  |

So today you can write:

```riven
struct Vec[T, const N: USize] end
let v: Vec[Int, 4] = ...
```

… and the parser will happily produce the AST, but the resolver
doesn't yet know that `4` is a const-generic argument.  This is the
nature of a feature implemented under SDD: each stage commits its
own slice of behaviour with its own pin tests, and you can read the
spec to know exactly what's wired.

---

## Reserved error codes

| Code  | Meaning                                              | Stage |
|-------|------------------------------------------------------|-------|
| E0700 | Kind mismatch on generic arg (type vs const)         | S5    |
| E0701 | Wrong const-arg type (`Bool` where `USize` expected) | S5    |
| E0702 | Non-const expression in const-arg position           | S2/S3 |
| E0703 | Const-arg expression overflows during evaluation     | S8    |

---

## Out of scope (matches Rust `min_const_generics`)

These are explicit non-goals — they won't ship even in v2.next:

- **Arbitrary `const fn`** (Zig's `comptime` model is a separate feature).
- **Floating-point const generics** (`NaN != NaN` breaks type equality).
- **`String` / `&str` const generics**.
- **Const-generic type parameters** (`fn foo[const T: Type]`).
- **Const-generic specialization**.
- **Defaults for const generics** (`const N: USize = 4`).
- **Inference of const arguments** (Rust says no in `min_const_generics`).

---

## How to read this chapter

The hardest part of using an in-flight feature is knowing where the
edge is.  Three rules:

1. **The spec's stage map is authoritative.**  If
   [`docs/specs/types/const-generics.spec.md`](../specs/types/const-generics.spec.md)
   says S3 is pending, you can rely on S2 working and S3 not.
2. **Pin tests are the contract.**
   `crates/riven-core/tests/const_generics.rs` shows every observable
   behaviour the compiler currently enforces.
3. **Don't write production code that needs S3+ behaviour.**  Use
   plain generics or a custom wrapper until the relevant stage lands.

---

## Tracking progress

The const-generics roadmap lives in three places, kept in sync:

- **Spec stage map** —
  [`docs/specs/types/const-generics.spec.md`](../specs/types/const-generics.spec.md).
- **Prompt 07 DoD** —
  [`docs/prompts/v1/07_phase3_const_generics.md`](../prompts/v1/07_phase3_const_generics.md).
- **Flukebase memory** — a procedural memory scoped to the `riven`
  project that records which stages have shipped + their commits.

When you pick up the next stage, update all three.

---

**Next:** [Chapter 14 — Foreign Function Interface](14-ffi.md) if you
want to call into C, or browse the [Spec index](../specs/README.md)
to see every area that has a formal contract.
