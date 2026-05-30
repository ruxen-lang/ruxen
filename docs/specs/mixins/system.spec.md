# Spec — Mixin system

**Source docs:**
[docs/requirements/tier2_01_assoc_types.md](../../requirements/tier2_01_assoc_types.md),
[docs/requirements/tier2_04_some_mixin_and_specialization.md](../../requirements/tier2_04_some_mixin_and_specialization.md),
[docs/requirements/tier2_06_any_mixin.md](../../requirements/tier2_06_any_mixin.md).

**Status:** shipped through Phase 2 #02-#04 plus Tier-2 surface
(assoc types, multi-bound, `any Mixin` existentials).

This spec covers Ruxen's user-facing mixin surface: declaring mixins,
adopting them with `include`, default methods, mixin inheritance,
associated types, generic constraints, and `any Mixin` parameters.
Implicit includes are spec'd separately in
[implicit_includes.spec.md](implicit_includes.spec.md).

---

## B1 — Mixin declaration with required method signature

```ruxen
mixin Greeter
  def greet -> String
end
```

The mixin declares a contract method; including classes must provide
a body.  No body required at the declaration site.

## B2 — `include Mixin` adopts the mixin in a class body

```ruxen
class Person
  name: String
  def init(@name: String) end

  include Greeter

  def greet -> String
    "hi, #{self.name}"
  end
end
```

The `include` directive lives inside the class body alongside the
method that satisfies the contract.  The compiler emits a
method-mangled MIR function (`Person_greet`) and dispatches the
mixin call site to it.

## B3 — Default methods in mixin body

```ruxen
mixin Greeter
  def name -> String
  def greet -> String
    "hello, #{self.name}"
  end
end
```

`name` is required; `greet` has a default that calls `name`.

## B4 — Including class can override default methods

The including class may redefine a defaulted method by writing its
own `def greet`.  The class's definition takes precedence over the
mixin's default body.

Stacked-mixin ambiguity: if two included mixins each provide a
default body for the same method name and the class itself defines
no override, the compiler rejects with `E-MIX-AMBIGUOUS-DEFAULT` and
requires the class to define its own implementation to disambiguate.
(`super` inside the override calls the mixin's default body; see
§3.4 of the canonical syntax spec.)

## B5 — Mixin inheritance: `mixin B: A`

```ruxen
mixin A
  def a -> Int
end
mixin B: A
  def b -> Int
end
```

A class that `include B` must also satisfy `A`'s contract — the
includer's body must provide both `a` and `b` (or include another
mixin / inherit from a class that provides them).  Methods from
both mixins are dispatchable on the including type.

## B6 — Associated types

```ruxen
mixin Container
  type Item
  def first(self) -> Self.Item
end
```

The including class binds `type Item` to a concrete type.  Return
type flows through.

## B7 — `some Mixin` parameter position

```ruxen
def print_it(x: &some Greeter)
  puts x.greet
end
```

`some Mixin` is monomorphized per call site — the compiler picks one
concrete conforming type per call, the function body is specialized
to that type, and methods may inline.  Zero runtime cost.
Structural satisfaction is accepted for `some Mixin`.

## B8 — `any Mixin` parameter position

```ruxen
def print_it(x: &any Greeter)
  puts x.greet
end
```

`any Mixin` is a fat pointer (data + vtable) at runtime; method
calls dispatch through the vtable.  One function body handles all
conforming types — required for heterogeneous collections.

`any Mixin` requires an explicit `include Greeter` directive in the
implementing class (structural match alone is not enough).  There
is no `&var some Mixin` or `&var any Mixin` form — to mutate
through an existential, take ownership (`Box[any Greeter]`,
`Shared[any Greeter]`, `SharedSync[any Greeter]`).

## B9 — Multi-bound generics

```ruxen
def show[T: Display + Debug](x: T) ...
```

The function may call methods from any of the bounded mixins on `x`.

## B10 — `where` clause syntax

```ruxen
def f[T](x: T) where T: Display + Hashable ...
```

Equivalent to inline `T: Display + Hashable` but allows more
elaborate bounds.  Per-method `where` clauses are written on the
`def` header; conditional methods on a generic type body live in
an `extension C[T] where T: B ... end` block (see §3.4a of the
canonical syntax spec).

## B11 — Mixin with a class-level method

```ruxen
mixin Empty
  def self.empty -> Self
end
```

`def self.empty` is a class-level constructor; callers use
`Bag.empty()`.  Class-level methods make the enclosing mixin
non-object-safe (it cannot appear as `any Empty`).

## B12 — Method resolution order: inherent → mixin → default

When a type has both an inherent method (defined in its own body)
and a mixin-provided method of the same name, the inherent method
wins.  When a mixin's default body and the class's override coexist,
the override wins.

---

## Pin tests

| Behaviour | Test fixture / fn                                  | File                                  |
|-----------|----------------------------------------------------|---------------------------------------|
| B1, B2    | `21_mixins.rx`                                    | `tests/release-e2e/cases/`            |
| B2        | `22_mixin_default.rx` (positive override)         | `tests/release-e2e/cases/`            |
| B3, B4    | `86_mixin_default_method_used.rx` + `87_mixin_override_default.rx` | `tests/release-e2e/cases/`  |
| B5        | `79_mixin_inherit.rx`                             | `tests/release-e2e/cases/`            |
| B6        | `80_mixin_assoc_type.rx`                          | `tests/release-e2e/cases/`            |
| B7        | `82_some_mixin_param.rx`                          | `tests/release-e2e/cases/`            |
| B8        | `83_any_mixin_param.rx`                           | `tests/release-e2e/cases/`            |
| B9        | `84_multi_bound.rx`                               | `tests/release-e2e/cases/`            |
| B10       | `100_where_clause.rx` + `103_generic_constraint.rx` | `tests/release-e2e/cases/`         |
| B11       | `81_mixin_static_method.rx`                       | `tests/release-e2e/cases/`            |
| B12       | covered transitively by B4 + `66_class_inline_include.rx` | `tests/release-e2e/cases/`    |

Dispatch correctness for implicitly-included structural mixins
(Debug / Clone / Eq / Hashable / Default / Ord / PartialOrd / Copy)
is covered by [implicit_includes.spec.md](implicit_includes.spec.md) B13.

<!-- TODO(migration): `E-MIX-AMBIGUOUS-DEFAULT` (B4) currently has no dedicated pin test; add when typeck enforces the new §3.4 rule. -->

---

## Gaps

- B12: no dedicated typeck pin asserting "inherent beats mixin"
  resolution order; today this is exercised only through composite
  E2E fixtures.

## Out of scope (v2)

- Higher-rank mixin bounds (`for[a] ...`).  Tracked in
  `tier2_03_hrtbs.md`.
- Generic associated types (`type Item[K]`).  Tier 2.5.
- Specialization (overlapping `extension` blocks where one is
  strictly more specific than another).
- `any` existentials over multiple non-auto mixins (`any Foo + Bar`).
- Const mixin methods.
