# Tier 2.02 — Const Generics

Status: Draft (requirements)
Owner: compiler
Depends on: monomorphization (M2 in overview); associated-type resolver
            improvements (doc 01 phase 01a) for parser reuse
Blocks: nothing on the tier-1 critical path; unblocks `SmallVec`,
        `Matrix[T, M, N]`, SIMD lane types, any fixed-size-buffer API

## 1. Summary & Motivation

A *const generic* is a generic parameter whose value is a compile-time
constant rather than a type. The canonical example:

```riven
struct Matrix[T, const M: USize, const N: USize]
  data: [T; M * N]
  def init(@data: [T; M * N]) end
end

def transpose[T, const M: USize, const N: USize](m: &Matrix[T, M, N]) -> Matrix[T, N, M]
  ...
end
```

The three pressure points:

1. **Array sizes that depend on a parameter.** Today `[T; N]` is
   `Ty::Array(Box<Ty>, usize)` — the `usize` is extracted once at
   resolve time from an integer literal
   (`resolve/mod.rs:2470-2472`) and cannot vary across instantiations.
   `SmallVec[T; N]` cannot be written.
2. **Type-level computation for fixed-size wrappers.** Matrix, vector,
   stack-allocated hashmap, bitset — all need `N` to flow from a user
   to a field type.
3. **SIMD lane width.** Any vectorised stdlib API would pass the lane
   width as a const generic (`fn sum[const N]([f32; N])`).

Const generics are a well-known cliff. Rust shipped `min_const_generics`
in 2021 (integer types, no expressions); the more general
`generic_const_exprs` is still unstable. Scala has none. C++ has the
full template-metaprogramming Turing-complete form (SFINAE etc.).
Zig's `comptime` is an alternative model — arbitrary compile-time
functions.

This doc specifies a **Riven-specific "min_const_generics + simple
arithmetic"** subset: integer and boolean-valued const parameters,
literal and parameter references, and the operators `+ - * /`. This
is enough for matrices, small-vectors, and SIMD, and stops short of
the quagmire of general const-fn evaluation.

## 2. Current State

### 2.1 `Ty::Array` already carries a `usize`

`hir/types.rs:73`:

```rust
Array(Box<Ty>, usize),
```

The size is a concrete number baked at resolve time. `resolve/mod.rs:2470`:

```rust
let n = if let Some(expr) = size {
    match &expr.kind {
        ExprKind::IntLiteral(v, _) => *v as usize,
        _ => 0,
    }
} else { 0 };
Ty::Array(Box::new(elem_ty), n)
```

Anything except an integer literal silently becomes `0`. No diagnostic.

### 2.2 `const` is a reserved keyword

`lexer/token.rs:135`: `Const`. `parser/mod.rs` parses a top-level
`const NAME: Ty = expr` item (`ast::ConstDef`, `parser/ast.rs:725-731`)
but the keyword is *not* accepted in generic-parameter positions.
`parser/types.rs:360-387` `parse_generic_param` only handles
`Lifetime` and `TypeIdentifier`.

### 2.3 Generic args are all types

`TypePath.generic_args: Option<Vec<TypeExpr>>` (`parser/ast.rs:49`).
There is no way to pass `3` as a generic argument.

### 2.4 No monomorphization

Every generic `Ty::Array(T, 0)` — because resolve gave up on the
expression — produces wrong layout at codegen. `codegen/layout.rs:312-319`:

```rust
Ty::Array(elem, n) => {
    let elem_layout = layout_of(elem, symbols);
    TypeLayout { size: elem_layout.size * n, ... }
}
```

`n == 0` → size `0` → the value occupies zero bytes at runtime. No
diagnostic.

### 2.5 No fixture uses const generics

No sample in `tests/fixtures/` has a `const`-parameterised item. The
tutorial chapter 12 doesn't mention const generics. **This is genuinely
new surface.**

## 3. Goals & Non-goals

### Goals

- **G1.** `struct Matrix[T, const M: USize, const N: USize]` parses,
  resolves, type-checks.
- **G2.** `[T; N]` where `N` is a const generic parameter resolves
  correctly, and each monomorphization has a concrete size.
- **G3.** Const-argument passing: `Matrix[Int, 3, 4]` parses and the
  resolver maps `M = 3, N = 4`.
- **G4.** Const parameters may appear in field types, method return
  types, and const-evaluated expressions in array sizes
  (`[T; M * N]`, `[T; N + 1]`).
- **G5.** Two instantiations with the same const arguments share a
  single monomorphized body (coherence).
- **G6.** Two instantiations with different const arguments have
  distinct types: `Matrix[Int, 3, 4]` and `Matrix[Int, 4, 3]` are
  **not** assignable to one another, even though `T` is identical.
- **G7.** `where` clauses accept const predicates: `where N > 0`,
  `where M == N`. Phase 02b; see §6.2.

### Non-goals

- **NG1.** Arbitrary compile-time functions. No `const fn`. No loops,
  no recursion, no match at the const level. This is Zig's `comptime`
  and is a different feature entirely.
- **NG2.** Floating-point const generics. Rust omits them because
  `NaN != NaN` breaks type equality. Riven follows suit.
- **NG3.** `String` or `&str` const generics. Rust is moving toward
  this slowly; not in tier 2.
- **NG4.** Const-generic type parameters: `fn foo[const T: Type]`.
  That's Scala's type functions, a separate feature.
- **NG5.** Const-generic specialization. Picking a different impl
  based on `N == 0`. Out of scope (see doc 04 §9 on specialization
  generally).
- **NG6.** Default values for const generics. `const N: USize = 4`.
  Defer; Rust has it but it's not load-bearing.

## 4. Surface Syntax

### 4.1 Declaration

```riven
struct Vector[T, const N: USize]
  data: [T; N]
end

class Matrix[T, const M: USize, const N: USize]
  data: [[T; N]; M]
  def init(@data: [[T; N]; M]) end
end

trait FixedBuffer[const CAP: USize]
  def capacity -> USize { CAP }
end

def rotate[const K: USize](arr: &[Int; 8]) -> [Int; 8] ...
```

Rules:

- Const parameters are introduced with the keyword `const` followed by
  a name and a type annotation. The type must be a built-in integer
  (`Int`, `Int8..64`, `UInt`, `UInt8..64`, `USize`, `ISize`) or `Bool`.
- Const parameters appear in the same brackets as type parameters,
  in any order. Canonical style: types first, consts after.
- A const parameter may appear anywhere a const expression is
  expected: array sizes `[T; N]`, other generic args `SmallVec[T, N]`,
  method bodies (`N` is in scope as a `USize` constant).

### 4.2 Instantiation

```riven
let v: Vector[Int, 4] = Vector.new([1, 2, 3, 4])
let m: Matrix[Float, 3, 3] = Matrix.zero
let r = rotate[3](&buf)
```

Rules:

- Const arguments are expressions that must be *const-evaluable* at
  the call/instantiation site (§5.2). In practice: integer literals,
  const-item references, or arithmetic on other in-scope const
  parameters.
- Turbofish-style explicit passing: `rotate[3](&buf)`. Mirrors the
  existing turbofish for type arguments. (Riven uses `[...]` for
  generics, not `::<...>` — see `parser/types.rs:317-335`.)

### 4.3 In signatures

```riven
struct SmallVec[T, const N: USize]
  data: [T; N]
  len: USize
end

impl[T, const N: USize] SmallVec[T, N]
  def init
    self.data = [T.default; N]
    self.len = 0
  end

  def push(item: T) -> Result[Unit, T]
    if self.len == N
      return Err(item)
    end
    self.data[self.len] = item
    self.len = self.len + 1
    Ok(())
  end

  def len -> USize { self.len }
  def capacity -> USize { N }
end
```

Rules:

- Impl header introduces `T` and `const N: USize` the same way the
  struct declaration does.
- `N` is in scope inside methods as an expression of type `USize`.

### 4.4 Arithmetic in array sizes

```riven
struct MatMul[const M: USize, const N: USize, const K: USize]
  out: [[Float; K]; M]
end

def concat[T, const A: USize, const B: USize](
  x: [T; A], y: [T; B]
) -> [T; A + B]
  ...
end
```

The grammar allowed in const expressions at type positions is the
subset:

```
const_expr := INT_LITERAL
            | CONST_PARAM_NAME
            | const_expr ('+' | '-' | '*' | '/') const_expr
            | '(' const_expr ')'
```

Division by zero is a compile-time error (E-CONST-DIV-ZERO). Overflow
is a compile-time error (E-CONST-OVERFLOW). No recursion, no branching.

Phase 02a supports only `literal` and `param`. Phase 02b adds `+ - * /`.

## 5. Type-System Changes

### 5.1 `Ty::Array` generalisation

Replace the concrete size with a const expression:

```rust
pub enum ConstExpr {
    Lit(u64),                  // integer literal (generalise when needed)
    Param(String),             // reference to a const generic param
    Op(Box<ConstExpr>, ConstOp, Box<ConstExpr>),
    Error,                     // recovery
}

pub enum ConstOp { Add, Sub, Mul, Div }

pub enum Ty {
    ...
    Array(Box<Ty>, ConstExpr),  // was (Box<Ty>, usize)
    ...
}
```

Placement: extend `hir/types.rs:73` and add `ConstExpr` near the top
of the file.

Equality rule: two `Ty::Array`s are type-equal iff elements unify *and*
the `ConstExpr`s evaluate to the same integer under the current
substitution. Concrete vs concrete → compare integers. Concrete vs
param → require the param to be equal, or normalize both through the
substitution map. Param vs param → structural equality of the
expression trees after a normal-form rewrite (see §5.4).

### 5.2 Const evaluator

A new module `hir/const_eval.rs`:

```rust
pub fn eval(expr: &ConstExpr, bindings: &HashMap<String, u64>)
    -> Result<u64, ConstEvalError>;
```

- `Lit(n)` → `n`.
- `Param(name)` → look up in `bindings`; if missing, return
  `Unresolved` (not an error — the caller may intend to leave the
  expression symbolic).
- `Op(a, op, b)` → recursively evaluate; apply the op with overflow
  check.

Used in:

- Monomorphization (M2): all param bindings known; eval must succeed
  or be a compile-time error.
- Layout (`codegen/layout.rs:312-319`): array layout evaluates the
  size; fails if any param is unbound (→ internal compiler error
  because monomorphization should have bound them).
- Where-clause checking (02b): `where N > 0` evaluates `N > 0` and
  rejects the instantiation if false.

### 5.3 Parser changes

`parse_generic_param` in `parser/types.rs:360`:

- If the current token is `Const`, consume it.
- Expect an identifier `N`, a `:`, a type (must be an integer or
  `Bool` primitive — enforce at resolve, not parser).
- Build a new AST variant:

```rust
pub enum GenericParam {
    Lifetime { name: String, span: Span },
    Type { name: String, bounds: Vec<TraitBound>, span: Span },
    Const { name: String, ty: TypeExpr, span: Span },  // NEW
}
```

`parse_generic_args` in `parser/types.rs:317`:

- After parsing each generic arg as a type, if the target parameter
  is a const param, re-parse the arg as a const expression. This
  requires knowing which kind each parameter is, which means the
  parser must do a second pass or the parser can parse every arg
  as "type-or-expr" and the resolver disambiguates.
- Recommendation: always parse as `TypeExpr` in the parser; add
  `TypeExpr::ConstLit(i64, Span)` as a variant that the parser emits
  when it sees an integer literal in a generic-arg position (simple
  lookahead: `[` seen, expecting a generic arg; if the first token
  is an integer literal, emit `ConstLit`). Resolve promotes `ConstLit`
  to `ConstExpr::Lit` for const parameters and errors for type
  parameters.

For phase 02b (arithmetic), the parser accepts general expressions in
generic-arg position behind a grammar flag; the parser already has
expression parsing, so this is an add-on.

### 5.4 Normal form for const expressions

For type equality, two expressions need a canonical form:

- Constant folding: `2 + 3` → `5`.
- Commutativity: `N + M` → lexicographic order on free variables.
- Associativity: right-fold `(a + b) + c` → `a + (b + c)`.
- Distributivity: do *not* apply (`N * (M + 1)` vs `N*M + N`) to
  avoid expensive equivalence.

This leaves a few false negatives (`N * (M + 1)` ≠ `N*M + N`), which
is the explicit Rust trade-off. Users write the same expression on
both sides, or pipe through a type alias.

Module: `hir/const_eval.rs::normalize(&ConstExpr) -> ConstExpr`.

### 5.5 Unification of const generics

`typeck/unify.rs::unify`:

- On `Ty::Array(a, n1)` vs `Ty::Array(b, n2)`: unify `a` and `b` as
  usual; require `normalize(n1) == normalize(n2)`.
- On `ConstExpr::Param(p)` vs `ConstExpr::Lit(v)`: bind `p → v` in the
  inference context (extend `hir/context.rs::TypeContext` with a
  `const_substitutions: HashMap<String, ConstExpr>`).
- On two `Param`s: either unify via the existing type-param path (if
  already bound) or register a deferred constraint.

### 5.6 Monomorphization dependencies

See overview §Dependency. Summary for this doc:

- The monomorphization pass (M2) enumerates per-instantiation keys
  as `(def_id, Vec<Ty>, Vec<ConstExpr after full eval>)`.
- Each distinct key produces one compiled function / type.
- Layout is computed per-instantiation with all const parameters
  bound.
- No generic function body refers to a still-unbound const parameter
  after M2.

### 5.7 Type-parameter order in printing / mangling

The mangler (part of M2) must include const args in its output:

- `Matrix_Int_3_4` for `Matrix[Int, 3, 4]`.
- Escape negative/large values: `Matrix_Int_NEG3_4`.
- Collide-check: `Matrix[Int, 3, 4]` and `Matrix[Int, 4, 3]` must not
  produce identical mangled names.

## 6. Implementation Plan

### 6.1 Code map

| Change | File(s) |
|---|---|
| `ConstExpr` + `ConstOp` | new section in `hir/types.rs` |
| `Ty::Array` carries `ConstExpr` | `hir/types.rs:73` and every match arm (layout, codegen, display) |
| Const evaluator + normalizer | new `hir/const_eval.rs` |
| `GenericParam::Const` | `parser/ast.rs:113-123` |
| `TypeExpr::ConstLit` | `parser/ast.rs:53-95` |
| Parse `const` in generic params | `parser/types.rs:360-387` |
| Parse integer literal in generic-arg position | `parser/types.rs:317-335` |
| Resolve const generics to `DefKind::ConstParam` | `resolve/symbols.rs`, `resolve/mod.rs` |
| `GenericParamInfo::Const` | `resolve/symbols.rs:22-26` |
| `TypeContext::const_substitutions` | `hir/context.rs:17-32` |
| Const-aware unification | `typeck/unify.rs` |
| Layout resolves `ConstExpr` | `codegen/layout.rs:312-319` |
| Mangler includes consts | new M2 code (overview) |
| Where-clause const predicates | `parser/types.rs:430-441`, `typeck/` |
| Error codes | `diagnostics/` |

### 6.2 Phasing

**Phase 02a — basic const params (2 weeks, blocks on M2 for codegen).**

1. Grammar: `const N: Type` in generic-param positions.
2. HIR: `ConstExpr` + `Ty::Array(_, ConstExpr)`.
3. Resolve: record `DefKind::ConstParam { ty }`; bind `N` in scope as
   a compile-time constant.
4. `parse_generic_args` accepts integer literals in generic-arg
   positions.
5. Type-check: `Vector[Int, 4]` unifies with `Vector[Int, N]` where
   `N = 4`.
6. Layout: arrays resolve size via the evaluator.
7. Monomorphization (M2): emit one body per (type args, const args)
   pair.
8. Error codes: E-CONST-TYPE-MISMATCH, E-CONST-OVERFLOW,
   E-CONST-DIV-ZERO (reserved; relevant in 02b).

**Phase 02b — arithmetic in const exprs (2-3 weeks).**

9. Grammar: accept `+ - * /` and parens in generic-arg position and
   in array-size position.
10. Normal-form rewriter.
11. Const evaluator supports arithmetic with checked overflow.
12. Constraint propagation: `[T; N]` + `N = M + 1` → concat case.
13. Where-clause const predicates: `where N > 0`, `where N == M`.
    Evaluated at monomorphization. Failing predicate → E-CONST-WHERE-FALSE.

## 7. Interactions With Other Tier-2 Features

### 7.1 With associated types (doc 01)

None directly. A trait with a const generic and an associated type is
legal; the two live in separate namespaces
(`TraitInfo.generic_params` gains `Const` entries; `assoc_types`
unchanged).

### 7.2 With GATs (doc 05)

Const generics on a GAT are legal but unmotivated by any concrete use
case. Permit them syntactically; no special-case logic.

### 7.3 With trait objects (doc 06)

`dyn Trait[const N: USize]` requires `N` to be bound at the
use site, same as associated types (doc 06 §4). `dyn FixedBuffer[4]`
is object-safe; bare `dyn FixedBuffer` is not (the vtable would need
to include the constant, which makes no sense — the constant is not
a runtime value).

### 7.4 With variance (doc 07)

Const parameters are *invariant*. `Matrix[T, 3, 4]` is neither a
subtype nor a supertype of `Matrix[T, 4, 3]`. This is the same rule
as for type parameters in invariant position and is naturally
expressed by the variance framework (doc 07 §4): const params are a
separate slot in the per-type variance table, always `Invariant`.

### 7.5 With HRTBs (doc 03)

No interaction. Const parameters have no lifetime component.

### 7.6 With specialization (doc 04 §9)

If specialization ships: `impl SmallVec[T, 0]` could override the
generic `impl[const N] SmallVec[T, N]` for the zero case. This is a
legitimate use and is the single motivation for specialization plus
const generics. However, see doc 04 §9 recommendation to defer
specialization; without it, users write a separate type `EmptySmallVec`.

## 8. Phasing

See §6.2.

## 9. Open Questions & Risks

- **OQ-1: what integer types?** Rust allows every integer type
  including `i128`. Riven has the same menu (`Int8..64`, `UInt8..64`,
  `USize`, `ISize`, `Int`, `UInt`). Recommendation: permit all.
  The evaluator uses `u128` internally; the signed/unsigned
  distinction affects overflow detection only.
- **OQ-2: `Bool` const generics.** `struct Sorted[T, const ASC: Bool]`
  is occasionally useful but adds a second dimension to the normal
  form. Recommendation: permit; it's a single extra `ConstExpr::Bool`
  variant.
- **OQ-3: inference of const arguments.** Can the user omit `[4]` in
  `Vector.new([1, 2, 3, 4])` and have `N = 4` inferred? Rust says no
  for const args; inference is restricted to type args in
  `min_const_generics`. Recommendation: follow Rust — inference is
  easy to add later but hard to remove.
- **OQ-4: ergonomics for array construction.** `[T.default; N]`
  requires `T: Default`. Without specialization, this is a hard
  bound. Where does `Default` come from? Tier-1 doc 01 phase 1a adds
  it to the stdlib. Cross-reference.
- **OQ-5: signed const params and underflow.** `N - M` where `N < M`.
  In unsigned, this is a compile error; in signed, it's fine.
  Recommendation: overflow-check everything; reject wraparound at
  monomorphization time.
- **OQ-6: default const params.** Left out per NG6. Revisit if stdlib
  wants `SmallVec[T]` to default to `N = 8`.
- **R-1: test matrix explosion.** Each const instantiation is a new
  monomorphization; a `Vec[Matrix[Float, M, N]]` with varying M, N
  multiplies compile cost linearly. Mitigation: document in the
  style guide that const generics should be used sparingly for
  hot-loop types, and never on public API where users can pick
  arbitrary values.
- **R-2: codegen regression on `[T; 0]`.** Today layout for `[T; 0]`
  is `size = 0` (layout.rs:312), which is correct. After phase 02a,
  `N = 0` still gives size 0; confirm no regressions in drop-glue
  (which must not iterate a 0-length array) and in `repr(C)`
  compatibility.
- **R-3: interaction with the "parse into `usize`" fallback at
  `resolve/mod.rs:2470-2472`.** The silent `_ => 0` must be turned
  into an error once `ConstExpr` replaces it. Expect fixture churn
  in any sample that accidentally typed `[T; x]` where `x` was a
  runtime variable.

## 10. Test Matrix

### 10.1 Positive tests

- T1: `struct Vector[T, const N: USize]` with `data: [T; N]`.
  Instantiate as `Vector[Int, 3]`; verify layout = 24 bytes (3 × 8).
- T2: generic function `fn sum[const N: USize](arr: &[Int; N]) -> Int`
  called with `&[1, 2, 3]` (N inferred from context, or explicit).
- T3: `SmallVec[T, N].push` errors when `self.len == N`.
- T4: monomorphization: `Vector[Int, 3]` and `Vector[Int, 4]` are
  distinct types; assignment between them fails.
- T5 (02b): `[T; A + B]` in `concat[A, B]`. Call with `A = 2, B = 3`;
  return type `[T; 5]`.
- T6 (02b): `where N > 0` on `fn head[T, const N: USize](arr: &[T; N]) -> &T`.
  Call with `N = 0` → E-CONST-WHERE-FALSE.

### 10.2 Negative tests

- N1: `const N: Float` → E-CONST-BAD-TYPE (only integers / bool).
- N2: const arg is a runtime variable → E-CONST-NONCONST.
- N3: `[T; N * 0 + 0]` vs `[T; 0]` — equal after normalization; check
  that unification succeeds.
- N4: `[T; N / 0]` → E-CONST-DIV-ZERO at monomorphization.
- N5: overflow: `const N: UInt8` with arg `300` → E-CONST-OVERFLOW.
- N6 (02b): `[T; N]` vs `[T; N + 0]` — must unify (normal-form
  folds).
- N7 (02b): `[T; N * (M + 1)]` vs `[T; N*M + N]` — **expected to
  fail** (documented limitation). Exact wording in error: "non-linear
  const expressions must be written in the same canonical form."

### 10.3 Fixture additions

- `tests/fixtures/const_basic.rvn` — SmallVec with fixed capacity.
- `tests/fixtures/const_matrix.rvn` — Matrix[T, M, N] with transpose.
- `tests/fixtures/const_inference.rvn` — array size inferred from
  literal argument.
- `tests/fixtures/const_error_overflow.rvn` — negative test.
