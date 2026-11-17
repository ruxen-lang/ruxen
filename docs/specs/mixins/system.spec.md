# Spec — Mixin system

**Source docs:**
[docs/requirements/tier2_01_assoc_types.md](../../requirements/tier2_01_assoc_types.md),
[docs/requirements/tier2_04_some_mixin_and_specialization.md](../../requirements/tier2_04_some_mixin_and_specialization.md),
[docs/requirements/tier2_06_any_mixin.md](../../requirements/tier2_06_any_mixin.md).

**Status:** shipped through Phase 2 #02-#04 plus Tier-2 surface
(assoc types, multi-bound, `any Mixin` existentials) [implemented].

Spelling today: a mixin is declared with `mixin Name ... end`, which is
[deprecated]; the Ruby form is `module Name ... end` with `include Name`
[planned](../../../TODO.md#t-module-mixins). Generic bounds are written `[T: Bound]`
today, also [deprecated]; the RBS form `[T < Bound]` is
[planned](../../../TODO.md#t-d7-respell). The examples below are the working spellings.
Owner decision D5 a: an `include` keeps its generic arguments in HIR
(`include Into[X]` loses `X` today) [planned](../../../TODO.md#t-d5).

This spec covers Ruxen's user-facing mixin surface: declaring mixins,
adopting them with `include`, default methods, mixin inheritance,
associated types, generic constraints, and `any Mixin` parameters.
Implicit includes are spec'd separately in
[implicit_includes.spec.md](implicit_includes.spec.md).

---

## B1 — Mixin declaration with required method signature

Works today, [deprecated] `mixin` (replacement `module`, [planned](../../../TODO.md#t-module-mixins)):

```ruby
mixin Greeter
  def greet -> String
end
```

The mixin declares a contract method; including classes must provide
a body.  No body required at the declaration site.

## B2 — `include Mixin` adopts the mixin in a class body

Works today, with the [deprecated] `def init` (replacement `def initialize`, [planned](../../../TODO.md#t-initialize)):

```ruby
class Person
  name: String
  def init(@name: String)
  end

  include Greeter

  def greet -> String
    "hi, #{@name}"
  end
end
```

The `include` directive lives inside the class body alongside the
method that satisfies the contract.  The compiler emits a
method-mangled MIR function (`Person_greet`) and dispatches the
mixin call site to it.

## B3 — Default methods in mixin body

A default calls another method of the mixin as `self.name` [implemented]; a
bare `name` call there does not compile yet [planned](../../../TODO.md#t-implicit-self).

Works today, [deprecated] `mixin` (replacement `module`, [planned](../../../TODO.md#t-module-mixins)):

```ruby
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

Stacked-mixin ambiguity: the target is Ruby's rule, the module included
last wins and the class's own method beats both [planned](../../../TODO.md#t-oq) (OQ-7).
The old draft rejected the program with `E-MIX-AMBIGUOUS-DEFAULT`; that name
was never registered. `super` inside an override, reaching the mixin's
default body, is [planned](../../../TODO.md#t-super) (E1127 outside `init` today).

## B5 — Mixin inheritance: `mixin B: A`

`mixin B: A` is [deprecated]; the Ruby form is `include A` inside `module B`
[planned](../../../TODO.md#t-module-mixins).

Works today, [deprecated] `mixin` and `mixin B: A` (replacements above):

```ruby
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

`type Item` in a mixin and `type Item = Int` in the class are [implemented]
(OQ-11 asks whether they stay).

Works today, [deprecated] `mixin` (replacement `module`, [planned](../../../TODO.md#t-module-mixins)):

```ruby
mixin Container
  type Item
  def first -> Self.Item
end
```

The including class binds `type Item` to a concrete type.  Return
type flows through.

## B7 — `some Mixin` parameter position

Example [implemented] (OQ-11; `some Mixin` as a return type is not accepted yet):

```ruby
def print_it(x: &some Greeter)
  puts x.greet
end
```

`some Mixin` is monomorphized per call site — the compiler picks one
concrete conforming type per call, the function body is specialized
to that type, and methods may inline.  Zero runtime cost.
Structural satisfaction is accepted for `some Mixin`.

## B8 — `any Mixin` parameter position

Example [implemented] (OQ-11):

```ruby
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
through an existential, take ownership (`Box[any Greeter]` or
`SharedSync[any Greeter]` [implemented]; `Shared[any Greeter]` once `Shared`
exists [planned](../../../TODO.md#t-shared)).

## B9 — Multi-bound generics

The Ruby form is `[T < A & B]` [planned](../../../TODO.md#t-d7-respell), and the
Rust-named `Display` and `Debug` give way to `to_s` and `inspect`
[planned](../../../TODO.md#t-d6).

Works today, [deprecated] `[T: A + B]` (replacement above):

```ruby
def show[T: Display + Debug](x: T) ...
```

The function may call methods from any of the bounded mixins on `x`.

## B10 — `where` clause syntax

Works today, [deprecated] `where` (replacement: bounds in the `[...]` list, [planned](../../../TODO.md#t-d7-respell)):

```ruby
def f[T](x: T) where T: Display + Hashable ...
```

Equivalent to inline `T: Display + Hashable` but allows more
elaborate bounds.  Per-method `where` clauses are written on the
`def` header; a method `where` bound on a class's type parameter
(`def sum -> T where T: Add`) is the form that works today for that case
(owner decision D7 b). Conditional methods on a generic type used to live in
an `extension C[T] where T: B ... end` block, which is [deprecated]; the
Ruby form is a bounded reopen, `class Box[T < Comparable] ... end`
[planned](../../../TODO.md#t-oq) (OQ-6).

## B11 — Mixin with a class-level method

Works today, [deprecated] `mixin` (replacement `module`, [planned](../../../TODO.md#t-module-mixins)):

```ruby
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
