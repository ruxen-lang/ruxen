# Spec — Const generics

**Source docs:**
[docs/requirements/tier2_02_const_generics.md](../../requirements/tier2_02_const_generics.md),
[docs/prompts/v1/07_phase3_const_generics.md](../../prompts/v1/07_phase3_const_generics.md).

**Status:** in flight.  Stages tracked individually below.

Const generics let types and functions be parameterised by a
compile-time **value** (integer or bool) rather than a type.  The
canonical examples are `Array[T, N: Int]`, `BitSet[N: USize]`, and
`SmallVec[T, N: USize]` — fixed-size collections whose layout
depends on a numeric parameter known at compile time.

Riven's design follows Rust's `min_const_generics` plus simple
arithmetic on const expressions.  No general `const fn` evaluation;
no recursion or branching at the const level.

---

## Stage map (top-level)

| Stage | Scope                                                            | Status      |
|-------|------------------------------------------------------------------|-------------|
| S1    | Parser accepts `const N: Type` in generic-param positions        | shipped (b8a371c) |
| S2    | Parser accepts integer literals as generic args at use sites     | shipped (bde3e1f) |
| S3    | Resolver records `DefKind::ConstParam { ty }`; brings `N` into scope inside the type / fn body | shipped (3d2d7ac) |
| S4    | HIR: `Ty::Array(Box<Ty>, ConstExpr)` + `ConstExpr` enum          | shipped (e2d073b) |
| S5    | Typeck: const args participate in unification; distinct const args produce distinct types | shipped (afac3fc) |
| S6    | Monomorphization (M2): one MIR fn per `(type-args, const-args)` key | shipped (a9fcb19 — `Ty::ConstArg` distinguishes instantiations; per-key MIR fn lowering tracked separately) |
| S7    | Codegen layout: arrays evaluate `ConstExpr` to a concrete size; `alloca` honours it | shipped (bc6cca3 — evaluator + `layout_of` integration; per-instantiation binding threading is a follow-up) |
| S8    | Arithmetic in const exprs (`+ - * /`), normal-form rewriter, overflow + div-zero diagnostics | shipped: S8.S1 evaluator (c0ea652), S8.S2 array-size resolve fold (5fd2c6d), S8.S3 const-arg position parser+resolve (ae22ed5), S8.S4 normal-form rewriter — overflow / div-zero E0703 surfacing is the monomorphization-side follow-up |
| S9    | `where` clause const predicates (`where N > 0`, `where N == M`)  | pending     |

### S7 follow-up — per-instantiation bindings through layout

The S7 evaluator integration sites all today's call site (`codegen/layout.rs::layout_of`) on an empty `&HashMap<String, u64>`.  This works for `[Int; 4]` (literal sizes), but a struct field `data: [T; N]` where `N` is a const param produces `Err(Unresolved("N"))` until the layout call learns to thread the per-instantiation binding map through.  Tracked as task before S9 lands, since `where`-clause predicates need the same plumbing.

Each stage commits its own spec+tests+impl trio.  Behaviours below
are tagged with the stage that lands them.

---

## B1 — `const N: Type` parses in generic-param declarations  *(S1)*

Generic-param brackets `[...]` accept a new form `const NAME: TYPE`
alongside the existing type and lifetime params.

**Given** the source
```riven
struct Vector[T, const N: USize]
  data: [T; N]
end
```
**When** the parser runs
**Then** the resulting `GenericParams` for `Vector` contains exactly
two entries: a `GenericParam::Type { name: "T", … }` followed by a
`GenericParam::Const { name: "N", ty: <TypeExpr USize>, … }`.

The same form is accepted on:
- `class T[..., const N: Type]`
- `enum T[..., const N: Type]`
- `def name[..., const N: Type](args)`
- `impl[..., const N: Type] T[..., N]`
- `trait T[..., const N: Type]`

Multiple const params and arbitrary ordering are accepted at the
parser layer (canonical style — types first, consts after — is a
formatter / lint concern, not a parser hard rule).

## B2 — Const-arg passing at use sites *(S2)*

Integer literals are accepted in generic-arg position when the
target parameter is a const.  Resolve promotes the literal to a
`ConstExpr::Lit`.

**Given** `Vector[Int, 4]` at a value position
**Then** the parser emits a `TypeExpr::Path` for `Vector` with the
second generic arg as `TypeExpr::ConstLit(4, …)` (a new TypeExpr
variant in S2).

## B3 — Const parameter is in scope in body *(S3)*

Inside a struct/class/enum body and methods, the const param name is
a compile-time constant of the declared type.

**Given** `class SmallVec[T, const N: USize] ... def capacity -> USize { N } end`
**When** typeck resolves the body
**Then** `N` is reachable as a `USize` value (no diagnostic), and
`def capacity` returns the const value at monomorphization time.

## B4 — `[T; N]` carries a `ConstExpr` *(S4)*

`Ty::Array` carries a `ConstExpr` rather than a concrete `usize`.
The expression tree captures whether the size is a literal
(`ConstExpr::Lit(n)`) or a parameter reference (`ConstExpr::Param("N")`).

## B5 — Two instantiations with different const args are distinct types *(S5)*

`Vector[Int, 3]` is not assignable to `Vector[Int, 4]` even though
the type arg `T` is identical.  Typeck rejects with a "mismatched
const generic argument" diagnostic.

## B6 — Same instantiations share one monomorphized body *(S6)*

Two call sites passing `Vector[Int, 3]` produce a single MIR function
for any methods involved; the mangler appends a `_3` suffix.

## B7 — Layout honours const-evaluated array sizes *(S7)*

`[Int; N]` where `N` is bound to `4` produces a 32-byte layout
(4 × 8); fixed-size arrays are stack-allocated via `alloca`.

## B8 — Arithmetic in const exprs *(S8)*

`+ - * /` and parens are accepted in const-arg position and in
array-size position (`[T; M * N]`, `[T; A + B]`).  The const
evaluator and normal-form rewriter unify expressions like
`[T; N + 0]` with `[T; N]`.

| Diagnostic               | Trigger                                          |
|--------------------------|--------------------------------------------------|
| E-CONST-DIV-ZERO         | `[T; N / 0]` at monomorphization                 |
| E-CONST-OVERFLOW         | `const N: UInt8 = 300` (or arithmetic overflow)  |
| E-CONST-BAD-TYPE         | `const N: Float`                                 |
| E-CONST-NONCONST         | const-arg position contains a runtime variable   |
| E-CONST-WHERE-FALSE      | `where N > 0` evaluates to false at use site (S9)|
| E-CONST-NORMAL-FORM      | `[T; N * (M + 1)]` vs `[T; N*M + N]` — same value, distinct normal forms (documented limitation) |

## B9 — `where` clause const predicates *(S9)*

`where N > 0`, `where N == M`, `where N + M == 8` evaluated at
monomorphization; failing predicate emits E-CONST-WHERE-FALSE.

---

## Error code reservations

| Code  | Meaning                                              | Stage |
|-------|------------------------------------------------------|-------|
| E0701 | wrong const-arg type (e.g. `Bool` where `USize` expected) | S5 |
| E0702 | non-const expression in const-arg position           | S2/S3 |
| E0703 | const-arg expression overflows during evaluation     | S8    |
| E0704 | kind mismatch on generic arg (passed type where const expected, or vice versa) | S5 |

> **Note (2026-05-14):** the kind-mismatch slot was originally
> assigned E0700, but the typeck `iterator-sum requires Add`
> validator predates the spec and already squats on E0700.  The
> spec was amended to use E0704 for kind mismatch; iterator-sum
> retains E0700.  All current emit sites have been updated to the
> new code (see `crates/riven-core/src/resolve/mod.rs` and the
> `const_lit_against_type_param_emits_e0704` pin test).

(Specific E-CONST-* names from §B8 will map to the E07xx slot once
reserved in the registry.)

---

## Pin tests

### Stage 1

| Behaviour | Test fn                                                  | File                                  |
|-----------|----------------------------------------------------------|---------------------------------------|
| B1 struct | `parse_const_generic_param_on_struct`                    | `crates/riven-core/tests/const_generics.rs` |
| B1 class  | `parse_const_generic_param_on_class`                     | `crates/riven-core/tests/const_generics.rs` |
| B1 fn     | `parse_const_generic_param_on_fn`                        | `crates/riven-core/tests/const_generics.rs` |
| B1 multi  | `parse_multiple_const_generic_params_typecheck_position` | `crates/riven-core/tests/const_generics.rs` |
| B1 mixed  | `parse_mixed_type_and_const_generic_params`              | `crates/riven-core/tests/const_generics.rs` |

### Stages 2-9

Pin tests added as each stage lands; see prompt-07 phasing notes.

---

## Out of scope (NG = non-goal from requirements §3)

- Arbitrary compile-time functions (`const fn`).  This is Zig's
  `comptime` and is a different feature entirely. (NG1)
- Floating-point const generics.  `NaN != NaN` breaks type equality. (NG2)
- `String` / `&str` const generics. (NG3)
- Const-generic type parameters (`fn foo[const T: Type]`). (NG4)
- Const-generic specialization (`impl SmallVec[T, 0]` overriding the
  generic impl). (NG5)
- Default values for const generics (`const N: USize = 4`). (NG6)
- Inference of const arguments — must always be written explicitly
  at the use site, matching Rust's `min_const_generics`. (OQ-3)
