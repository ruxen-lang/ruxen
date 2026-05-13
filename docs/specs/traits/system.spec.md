# Spec — Trait system

**Source docs:**
[docs/requirements/tier2_01_assoc_types.md](../../requirements/tier2_01_assoc_types.md),
[docs/requirements/tier2_04_impl_trait_and_specialization.md](../../requirements/tier2_04_impl_trait_and_specialization.md),
[docs/requirements/tier2_06_trait_objects.md](../../requirements/tier2_06_trait_objects.md).

**Status:** shipped through Phase 2 #02-#04 plus Tier-2 surface
(assoc types, multi-bound, trait objects).

This spec covers Riven's user-facing trait surface: declaring traits,
implementing them, default methods, trait inheritance, associated
types, generic constraints, and `dyn`-trait parameters.  Derive
macros are spec'd separately in [derive.spec.md](derive.spec.md).

---

## B1 — Trait declaration with abstract method signature

```riven
trait Greeter
  def greet -> String
end
```

The trait declares a method other types may implement.  No body
required.

## B2 — `impl Trait for Type` provides the method

```riven
impl Greeter for Person
  def greet -> String
    "hi, #{self.name}"
  end
end
```

The compiler emits a method-mangled MIR function (`Person_greet`)
and dispatches the trait call site to it.

## B3 — Default methods in trait body

```riven
trait Greeter
  def name -> String
  def greet -> String
    "hello, #{self.name}"
  end
end
```

`name` is required; `greet` has a default that calls `name`.

## B4 — Implementor can override default methods

The implementor may redefine a defaulted method.  Override takes
precedence over the trait's default.

## B5 — Trait inheritance: `trait B : A`

```riven
trait A
  def a -> Int
end
trait B : A
  def b -> Int
end
```

A type that `impl B for T`s must also `impl A for T`.  Methods from
both traits are dispatchable on `T`.

## B6 — Associated types

```riven
trait Container
  type Item
  def first(self) -> Self::Item
end
```

The implementor binds `type Item` to a concrete type.  Return type
flows through.

## B7 — `impl Trait` parameter position

```riven
def print_it(x: &impl Greeter)
  puts x.greet
end
```

`impl Greeter` is an anonymous generic parameter with a single
`Greeter` bound.

## B8 — `dyn Trait` parameter position

```riven
def print_it(x: &dyn Greeter)
  puts x.greet
end
```

`dyn Trait` is a fat pointer (data + vtable) at runtime; method calls
dispatch through the vtable.

## B9 — Multi-bound generics

```riven
def show[T: Display + Debug](x: T) ...
```

The function may call methods from any of the bounded traits on `x`.

## B10 — `where` clause syntax

```riven
def f[T](x: T) where T: Display + Hash ...
```

Equivalent to inline `T: Display + Hash` but allows more elaborate
bounds.

## B11 — Trait with a static method

```riven
trait Empty
  def self.empty -> Self
end
```

`self.empty` is a class-level constructor; callers use
`Bag::empty()`.

## B12 — Method resolution order: inherent → trait → default

When a type has both an inherent method and a trait method of the
same name, the inherent method wins.  When a trait's default
implementation and a user's override exist, the override wins.

---

## Pin tests

| Behaviour | Test fixture / fn                                  | File                                  |
|-----------|----------------------------------------------------|---------------------------------------|
| B1, B2    | `21_traits.rvn`                                    | `tests/release-e2e/cases/`            |
| B2        | `22_trait_default.rvn` (positive override)         | `tests/release-e2e/cases/`            |
| B3, B4    | `86_trait_default_method_used.rvn` + `87_trait_override_default.rvn` | `tests/release-e2e/cases/`  |
| B5        | `79_trait_inherit.rvn`                             | `tests/release-e2e/cases/`            |
| B6        | `80_trait_assoc_type.rvn`                          | `tests/release-e2e/cases/`            |
| B7        | `82_impl_trait_param.rvn`                          | `tests/release-e2e/cases/`            |
| B8        | `83_dyn_trait_param.rvn`                           | `tests/release-e2e/cases/`            |
| B9        | `84_multi_bound.rvn`                               | `tests/release-e2e/cases/`            |
| B10       | `100_where_clause.rvn` + `103_generic_constraint.rvn` | `tests/release-e2e/cases/`         |
| B11       | `81_trait_static_method.rvn`                       | `tests/release-e2e/cases/`            |
| B12       | covered transitively by B4 + `66_class_inline_impl.rvn` | `tests/release-e2e/cases/`       |

Trait-dispatch correctness for derive-generated impls covered by
[derive.spec.md](derive.spec.md) B13.

---

## Gaps

- B12: no dedicated typeck pin asserting "inherent beats trait"
  resolution order; today this is exercised only through composite
  E2E fixtures.

## Out of scope (v2)

- Higher-rank trait bounds (`for<'a> ...`).  Tracked in
  `tier2_03_hrtbs.md`.
- Generic associated types (`type Item[K]`).  Tier 2.5.
- Specialization (`impl<T: Foo> Bar for T` overlap).
- Trait objects with multiple non-auto traits (`dyn Foo + Bar`).
- Const trait methods.
