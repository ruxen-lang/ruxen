# Tier 2.03 — Higher-Ranked Trait Bounds (HRTBs)

Status: Draft (requirements)
Owner: compiler
Depends on: named lifetimes (M0 in overview); variance (M1 / doc 07)
Blocks: fluent closure APIs that accept any input lifetime;
        `impl Fn(&'_ T) -> &'_ T` style returns at the boundary

## 1. Summary & Motivation

A higher-ranked trait bound says "for any lifetime 'a, this type
satisfies the bound." The canonical example:

```riven
def filter_all[F: for['a] Fn(&'a Str) -> Bool](
  list: &Vec[String], f: F
) -> Vec[&Str]
  list.iter.filter(f).to_vec
end
```

Without HRTBs, the function signature would have to expose `'a` as a
parameter of `filter_all`, and the caller would be on the hook to
prove every borrow they hand to `f` lives long enough. HRTBs move the
quantifier inside the bound: *the closure works for any lifetime I
throw at it.*

**Why Riven needs this:**

1. **Closure adapters.** Any function that forwards a borrow to a
   user-supplied closure with a lifetime shorter than the function's
   body (`Vec.iter.filter(|x| predicate(x))`) needs the closure to
   be polymorphic in the argument lifetime — that's HRTBs.
2. **Trait-object closures.** `Box[dyn Fn(&Str) -> Bool]` is
   sugar for `Box[dyn for['a] Fn(&'a Str) -> Bool]`. Without HRTBs
   the elided syntax has no elaboration target.
3. **Stdlib methods that take an `FnMut` over references.** Every
   iterator combinator that takes `&T` (e.g., `.find`, `.any`, `.all`,
   `.position`) wants HRTB-qualified closure bounds.

**Scope decision for this doc.** HRTBs are orthogonal to associated
types and GATs. The 80-90% use case is HRTBs on `Fn`/`FnMut`/`FnOnce`
bounds. Broader HRTBs (`for['a] Iterator[Item = &'a T]`) interact with
GATs and are discussed there; this doc specifies the closure case
first and leaves the Iterator case as a 03b phase.

## 2. Current State

### 2.1 Lifetime tokens parse; lifetime generics resolve to nothing

`parser/types.rs:360-368` handles `GenericParam::Lifetime { name,
span }`. `resolve/mod.rs:2679-2682`:

```rust
ast::GenericParam::Lifetime { .. } => {
    // Lifetimes are tracked but not yet used in Phase 3
    None
}
```

Explicit. The parser accepts lifetime parameters; the resolver
silently discards them; the borrow checker infers everything by
elision or gives up.

### 2.2 Reference types carry an optional lifetime

`hir/types.rs:92-95`:

```rust
RefLifetime(std::string::String, Box<Ty>),
RefMutLifetime(std::string::String, Box<Ty>),
```

Parsed at `parser/types.rs:101-139`. `'a` is captured as a
`std::string::String`. The borrow checker does not use it.

### 2.3 Elision exists and handles most cases

`borrow_check/lifetimes.rs:55-85` implements Rust's three elision
rules:

- Rule 1: every input ref gets a distinct lifetime (implicit).
- Rule 2: exactly one input ref → output gets that lifetime.
- Rule 3: `&self` or `&mut self` → output gets `self`'s lifetime.

**What Riven currently *cannot* express:**

- "All inputs have the same lifetime" — no named lifetime parameter.
- "Output lifetime is this specific input's" — ambiguous elision is
  rejected with `E: AmbiguousOutputLifetime`.
- "Works for any lifetime" — the HRTB this doc is about.

### 2.4 Closures do not carry lifetime signatures

`HirExprKind::Closure { params, body, captures, is_move }`
(`hir/nodes.rs:168-173`) has no concept of "the param borrow has a
lifetime." Closures are lowered to a plain `Fn` type
(`resolve/mod.rs:2478-2482`), stripping any lifetime information.

### 2.5 `for` is a reserved keyword

`lexer/token.rs:93`: `For`. Currently only used for `for` loops
(`parser/expr.rs`). Reusing the keyword for HRTB syntax is safe
because `for[` or `for<'` is unambiguous with `for x in y`: the next
token is `[` / `<` not an identifier.

## 3. Goals & Non-goals

### Goals

- **G1.** Functions and trait bounds can be qualified by a universal
  quantifier: `for['a] F: Fn(&'a T) -> &'a T`.
- **G2.** The borrow checker correctly treats HRTB-qualified bounds:
  the closure may be called with *any* caller-chosen lifetime.
- **G3.** HRTBs compose with closures: `Fn(&Str) -> Bool` auto-elides
  to `for['a] Fn(&'a Str) -> Bool` at use-sites that require it.
- **G4.** HRTBs on `dyn Trait` are legal and produce the correct
  vtable (trivially — the vtable doesn't change; the type of the
  trait object does).
- **G5.** HRTBs are rare in user code — the stdlib and elision
  cover the common path. Explicit HRTB syntax is needed only when
  elision fails or the user wants to document intent.

### Non-goals

- **NG1.** Full predicative-polymorphic Rank-2 types (ML-style).
  HRTBs are restricted to trait bounds; no `forall a. expr` at the
  term level.
- **NG2.** Negative HRTBs (`for['a] !Send`). Rust experimented and
  retreated.
- **NG3.** HRTBs on const generics. `for<const N>` is not a thing.
- **NG4.** HRTBs at the type alias level (`type Lens['a] = ...`) —
  those are type constructors and belong in GATs (doc 05).
- **NG5.** Higher-kinded types. `T['a]` as a parametric type
  constructor in a trait bound. Out of scope entirely.

## 4. Surface Syntax

### 4.1 Two forms

**Full form:**

```riven
def each_line[F: for['a] Fn(&'a Str)](source: &String, f: F) ...
```

**Elided form:**

```riven
def each_line[F: Fn(&Str)](source: &String, f: F) ...
```

The elided form's `&Str` is equivalent to `for['a] &'a Str` *when
the closure's lifetime does not escape the signature* — which for
`Fn(&Str)` passed to `each_line` is always. This is the same
elision rule Rust uses for closure-typed function arguments.

### 4.2 `for[...]` vs `for<...>`

Rust uses `for<'a>` because it predates const generics and type
params were `<...>`. Riven uses `[...]` consistently for generics
(`parser/types.rs:338`). Recommendation: `for['a]`. See overview
§Open #7 for the decision.

```riven
def apply_twice[F: for['a] Fn(&'a Int) -> Int](f: F) -> Int
  let x = 1
  let y = 2
  f(&x) + f(&y)
end
```

### 4.3 Multi-lifetime HRTBs

```riven
def both[F: for['a, 'b] Fn(&'a Str, &'b Str) -> Bool](f: F) ...
```

Rare but legal. The quantifier binds both lifetimes.

### 4.4 HRTBs on trait objects

```riven
type LineProcessor = Box[dyn for['a] Fn(&'a Str) -> Result[(), Error]]

# Elided equivalent:
type LineProcessor = Box[dyn Fn(&Str) -> Result[(), Error]]
```

### 4.5 HRTBs in trait bodies

```riven
trait Parser
  def parse[R](self, input: &Str, reducer: impl for['a] Fn(&'a Str) -> R) -> R
end
```

Legal. Rare because `impl Trait` in argument position (doc 04) is the
usual idiom and HRTB qualification is implicit for closure types.

## 5. Elision Extension (the carrying-your-weight section)

Riven's elision rules currently cover most idioms that would otherwise
need HRTBs. Before adding HRTB syntax, extend the elision rules so
that **explicit HRTBs become rare**. The extensions:

### 5.1 Closure-arg elision (rule 4)

For function parameters of type `impl Fn(...)` / `impl FnMut(...)` /
`impl FnOnce(...)` or `Fn(...)`, `FnMut(...)`, `FnOnce(...)`, each `&T` /
`&mut T` in the closure's signature gets a fresh universal
lifetime, quantified over the closure. Equivalent to implicitly
wrapping the bound in `for[...]`.

```riven
# Written:
def each[F: Fn(&Str)](items: &Vec[String], f: F) ...

# Elaborated:
def each[F: for['a] Fn(&'a Str)](items: &Vec[String], f: F) ...
```

This covers the majority of closure bounds in the stdlib.

### 5.2 Trait-object closure elision (rule 5)

`dyn Fn(&T)` and `Box[dyn Fn(&T)]` similarly elaborate to HRTBs.

### 5.3 When elision does *not* apply (and HRTBs are required)

- The closure's argument lifetime must equal a lifetime named
  elsewhere in the signature: explicit HRTBs (or named lifetimes) are
  needed.
- The closure's argument lifetime must *escape* through a return:
  `Fn(&'a Str) -> &'a Str` in a situation where `'a` is fixed to an
  input lifetime — again, named lifetimes.
- The user wants to write `for['a]` to document intent: permitted,
  not required.

### 5.4 Lifetime-elision rule 3 extension

Today rule 3 (`borrow_check/lifetimes.rs:63-64`) gives `&self` /
`&mut self` methods their output lifetime from self. Extend to:

- If the method has exactly one input reference of a *user-defined*
  reference type (a struct containing a reference, say), and one
  `&self`, prefer `self`'s lifetime for the return.
- If the method returns `impl Iterator[Item = &T]`, the `&T` gets
  the same lifetime as the receiver.

These extensions pay down HRTB demand without new syntax.

## 6. Type-System Changes

### 6.1 HIR `TraitRef` grows an optional HRTB quantifier

`hir/types.rs:14-18`:

```rust
pub struct TraitRef {
    pub name: String,
    pub generic_args: Vec<Ty>,
    pub assoc_eq: Vec<(String, Ty)>,  // doc 01
    pub higher_ranked: Vec<String>,    // NEW: ['a, 'b] bound at this trait ref
}
```

`higher_ranked` is empty for non-HRTB bounds. Non-empty: the listed
lifetime names are universally quantified over this specific trait
reference.

### 6.2 Named-lifetime infrastructure (M0 pre-flight)

The rest of the variance doc (07) and this doc both need named
lifetimes to actually *work*. The current state is: parse, discard.
The M0 work:

- `DefKind::Lifetime { name: String }` in the symbol table.
- `resolve_generic_params` populates these instead of returning
  `None`.
- `Ty::RefLifetime(name, ..)` / `Ty::RefMutLifetime(name, ..)`
  continue to be used; the borrow checker's existing `check_outlives`
  (`borrow_check/lifetimes.rs:100-112`) is upgraded to a relation on
  named lifetimes.

This is the pre-flight for every named-lifetime feature; it is
specified in the overview doc as M0 and will be fleshed out in doc 07
§M0.

### 6.3 Parser changes

`parse_single_trait_bound` (`parser/types.rs:400-405`):

- If the current token is `For`, consume it; expect `[`; parse a
  comma-separated list of `Lifetime`; expect `]`; then parse the
  underlying bound.
- Store the quantifier list in the `TraitBound` AST variant:

```rust
pub struct TraitBound {
    pub path: TypePath,
    pub assoc_eq: Vec<AssocEqConstraint>,  // doc 01
    pub higher_ranked: Vec<String>,         // NEW
    pub span: Span,
}
```

### 6.4 Borrow-check skolemization

When the borrow checker encounters a call to a closure `F: for['a]
Fn(&'a T) -> U`:

- Each call site of `F` introduces a *fresh skolem* lifetime for
  `'a`: `'a₁` at call site 1, `'a₂` at call site 2.
- The caller is responsible for ensuring the argument lives at least
  `'aᵢ` long.
- Because `'aᵢ` is fresh, the skolem is only known to outlive what
  the caller proves — not the function body itself. This is the
  correctness condition.

Implementation sketch: extend `borrow_check/regions.rs:ScopeStack`
with a `skolem_lifetimes: HashMap<(CallSite, String), ScopeId>` map;
bind at call entry, release at call exit.

### 6.5 Subtyping on HRTBs

For an HRTB-qualified bound, the implementer (the closure) must
satisfy the bound for every lifetime, not just one. The subtyping
rule: `F: for['a] Fn(&'a T) -> U` → `F: Fn(&'b T) -> U` for any
specific `'b`. This is a *decreasing* subtype — HRTB is strictly
stronger than a monomorphic closure bound. In the Rust terminology,
HRTBs are covariant in quantified lifetimes.

This rule is implemented in `typeck/unify.rs` as a special case of
the closure-bound check: when checking a closure argument against
a parameter's HRTB bound, instantiate the quantifier with a fresh
variable and unify.

### 6.6 No monomorphization impact

HRTBs do not multiply monomorphization keys. The same compiled body
works for every quantified lifetime because lifetimes are erased at
codegen. This is the big reason HRTBs are cheap to implement *once
you have named lifetimes.*

## 7. Implementation Plan

### 7.1 Code map

| Change | File(s) |
|---|---|
| AST `TraitBound.higher_ranked: Vec<String>` | `parser/ast.rs:100-104` |
| Parse `for['a, 'b] ...` in bounds | `parser/types.rs:400-405` |
| HIR `TraitRef.higher_ranked` | `hir/types.rs:14-18` |
| Resolve propagates quantifier through | `resolve/mod.rs` (trait-bound walker) |
| M0 lifetime infrastructure | `resolve/mod.rs:2679-2682`, `resolve/symbols.rs`, `borrow_check/regions.rs` |
| Closure-arg elision rule 4 | `borrow_check/lifetimes.rs:55-85` |
| Trait-object closure elision | `borrow_check/lifetimes.rs` + `resolve/mod.rs:2492-2499` |
| Skolem scope map | `borrow_check/regions.rs`, `borrow_check/mod.rs` |
| Closure-arg unification with HRTB | `typeck/unify.rs` |
| `dyn for['a] ...` prints/parses round-trip | `hir/types.rs::Display`, formatter |
| Error codes | `diagnostics/` |

### 7.2 Phasing

**Phase 03a — elision extensions (1 week, stands alone).**

1. Implement closure-arg elision (§5.1).
2. Implement trait-object closure elision (§5.2).
3. Extend rule 3 for receivers (§5.4).
4. Tests show the stdlib closures compile without explicit HRTBs.

This phase delivers *most* of the user-visible value of HRTBs
without a syntax change. Validation metric: every iterator
combinator in the tier-1 stdlib plan compiles with its intended
signature and no explicit lifetime annotations.

**Phase 03b — explicit HRTB syntax (1-2 weeks, depends on M0).**

5. Accept `for['a] ...` in the parser.
6. Propagate through resolve + HIR.
7. Borrow check with skolemization.
8. Error messages for HRTB violations.

**Phase 03c — advanced cases (deferred to post-tier-2).**

9. HRTBs on non-`Fn` traits: `T: for['a] Iterator[Item = &'a U]`.
   Requires GATs (doc 05) to express the quantified projection.
10. HRTBs with outlives bounds: `for['a: 'b] Fn(&'a T)`.
11. Impl-trait return with HRTB: `-> impl for['a] Fn(&'a T)`. Rare
    in practice, deferred.

## 8. Interactions With Other Tier-2 Features

### 8.1 With associated types (doc 01)

`for['a] I: Iterator[Item = &'a T]` moves the associated-type
projection inside the quantifier. Normalization must delay unfolding
the projection until the skolem is instantiated at a call site.
This is the "higher-ranked normalization" hazard Rust fought with.

Strategy: in 03b, reject HRTBs that mention associated types
(E-HRTB-ASSOC-UNSUPPORTED). Unblock in 03c / GATs doc 05 §6.

### 8.2 With GATs (doc 05)

GATs + HRTBs is the "streaming iterator" pattern:

```riven
trait StreamingIterator
  type Item['a]
  def mut next['a](self: &'a mut Self) -> Option[Self.Item['a]]
end

def take_three[S: for['a] StreamingIterator](s: &mut S) ...
```

This is doc 05 §7. The current doc treats it as an open interaction.

### 8.3 With trait objects (doc 06)

`dyn for['a] Fn(&'a T)` is object-safe iff `Fn` is object-safe
(it is — see doc 06 §4). The vtable layout is identical to `dyn Fn`;
the HRTB only affects the type-level quantification.

### 8.4 With variance (doc 07)

HRTBs *require* lifetime variance to work. The skolem lifetime must
be treated as covariant where the bound is in covariant position
(the closure's argument, for `Fn`) and contravariant where the bound
is in contravariant position (the return of `Fn`). Doc 07 §5
specifies the full variance table for closure types.

### 8.5 With specialization

No interaction. HRTBs and specialization live in different parts of
the solver.

### 8.6 With impl Trait (doc 04)

`impl for['a] Fn(&'a T) -> U` in argument position is equivalent to
a named type param plus the HRTB bound. Elision covers the common
case. Explicit HRTBs on `impl Trait` are legal.

`impl for['a] Fn(&'a T) -> U` in return position is rare; reject in
03b as unsupported, revisit later.

## 9. Open Questions & Risks

- **OQ-1: syntax.** `for['a]` vs `for<'a>`. See overview §Open #7.
  Recommendation: `for['a]`.
- **OQ-2: elision for traits beyond Fn-family.** Could any user
  trait with a `&T` parameter elide to an HRTB? Rust limits this to
  `Fn`-family. Recommendation: limit to `Fn`/`FnMut`/`FnOnce` in
  phase 03a. A general rule can be added later.
- **OQ-3: error messaging.** "The closure `|x| x.len` does not live
  long enough to satisfy `for['a] Fn(&'a Str) -> USize`" is a
  user-hostile message. Mitigate by explaining: "your closure
  captures `x` but the bound requires a closure that does not
  capture references tied to its input."
- **OQ-4: what if the user writes `for[]` (empty quantifier)?**
  Semantically equivalent to no HRTB. Accept and warn.
- **OQ-5: `for['static]`.** Redundant (`'static` is universally
  available). Reject with E-HRTB-STATIC.
- **R-1: elision magic is hard to teach.** Users who type
  `fn f[F: Fn(&Str)]` will be surprised that it compiles where
  `fn f[F: Fn(&'a Str)]` does not. Document in the tutorial.
- **R-2: borrow-check skolemization is a well-known source of
  compiler bugs in Rust.** Mitigation: test matrix N4-N7 (below)
  targets the known-hard cases.
- **R-3: performance of the HRTB solve.** Two separate
  instantiations of the same skolem can produce an exponential blow-
  up in pathological cases. Set a depth limit (say, 16) and fail
  with E-HRTB-SOLVER-DEPTH.

## 10. Test Matrix

### 10.1 Positive tests

- T1: `def each[F: Fn(&Str)](items: &Vec[String], f: F)` — elided
  HRTB. Call with a closure `|s| puts s`.
- T2: Explicit HRTB: `def each[F: for['a] Fn(&'a Str)](...)`.
  Verify equivalence to T1.
- T3: Multi-lifetime HRTB: `for['a, 'b] Fn(&'a Str, &'b Str) -> Bool`.
- T4: `dyn Fn(&Str) -> Bool` accepts a trait-object closure without
  explicit lifetimes.
- T5: `Box[dyn for['a] Fn(&'a Str) -> Bool]` stored in a field,
  invoked with a borrow.
- T6: HRTB on an `impl Trait` argument: `def each(items: &Vec[String],
  f: impl Fn(&Str))`.

### 10.2 Negative tests

- N1: closure returns a borrow of a local:
  `fn bad() -> Fn(&Str) -> &Str { |s| &String.from(s) }` →
  E-HRTB-RETURN-LOCAL.
- N2: HRTB where the quantified lifetime is also a parameter of the
  enclosing function → E-HRTB-SHADOW.
- N3: `for[]` with no lifetimes → warn (W-HRTB-EMPTY).
- N4 (skolemization): closure signature claims `for['a] Fn(&'a Str)
  -> &'a Str` but the implementation captures a `&'static Str` and
  returns it → must be accepted (the `'static` is a specific
  lifetime that *is* `'a` for every caller).
- N5 (skolemization): closure requires `F: for['a] Fn(&'a Str)`,
  but the caller passes `|s: &'outer Str| ...` where `'outer` is a
  specific parameter of the caller → E-HRTB-NOT-POLY.
- N6 (rule 4 elision): a closure that captures a borrow:
  `let outer = String.new; each(&items, |x| outer.contains(&x))` —
  verify that `outer` is bound by the closure's captures and the
  HRTB bound is satisfied.
- N7 (associated types): HRTB mentions an associated type in 03b →
  E-HRTB-ASSOC-UNSUPPORTED.

### 10.3 Fixture additions

- `tests/fixtures/hrtb_closure_arg.rvn` — elided; uses `Vec.filter`
  with a closure taking `&T`.
- `tests/fixtures/hrtb_explicit.rvn` — explicit `for['a]` syntax.
- `tests/fixtures/hrtb_dyn.rvn` — `Box[dyn Fn(&Str)]` stored in a
  struct and called repeatedly.
- `tests/fixtures/hrtb_error_non_poly.rvn` — negative: closure tied
  to a specific lifetime where HRTB was required.
