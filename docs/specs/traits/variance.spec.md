# Spec — Variance & coercion

**Source docs:**
[docs/requirements/tier2_07_variance.md](../../requirements/tier2_07_variance.md).

**Status:** shipped Tier-2 (`variance.rs` module wired into typeck).

Riven's typeck enforces variance rules at coercion / argument sites:
which "wider" types may flow where, which "narrower" types may not.
This spec lists the rules that have explicit pin tests.

---

## B1 — `&mut T` is **invariant** in `T`

`&mut Inner1` does **not** coerce to `&mut Inner2`, even when
`Inner2` is a supertype / wider type.

| Rejection                                              | Reason                       |
|--------------------------------------------------------|------------------------------|
| `&mut SubClass` → `&mut BaseClass`                     | unsound: write through alias |
| `&mut Int8`     → `&mut Int64`                         | width mismatch               |
| `&mut TypeA`    → `&mut UnrelatedTypeB`                | unrelated types              |

## B2 — `&mut T` → `&T` is allowed

The standard "mutable to immutable" reborrow remains legal; a
`&mut T` automatically demotes to `&T` at coercion sites.

## B3 — `Vec[T]` is **invariant** in `T`

`Vec[Inner1]` does not coerce to `Vec[Inner2]`, by the same logic
as B1 (a write through the wider alias would be unsound).

## B4 — `Vec[T]` of the same `T` is accepted

Sanity check that the invariance rule does not over-reject:
`Vec[Foo]` flows where `Vec[Foo]` is expected.

## B5 — `Option[T]` is **covariant** in `T`

Read-only single-value wrappers admit safe upcasts.

| Coercion accepted                                       | Reason                |
|---------------------------------------------------------|-----------------------|
| `Option[&mut T]` → `Option[&T]`                         | demote inner          |
| `Option[Int8]`   → `Option[Int64]`                      | integer widening      |

`Option[T]` does not coerce when the inner types are incompatible
(e.g. `Option[String]` → `Option[Int]`).

## B6 — `variance` module is wired into typeck

The variance checker runs as part of the typeck pipeline (not gated
behind a feature flag or test-only hook).  Sanity test asserts the
module is reachable.

---

## Pin tests

| Behaviour | Test fn                                               | File          |
|-----------|-------------------------------------------------------|---------------|
| B1        | `mut_ref_no_coerce_to_different_inner_type` + `mut_ref_no_coerce_widening_inner` + `mut_ref_no_coerce_to_different_class` | `variance.rs` |
| B2        | `mut_ref_to_immut_ref_still_works`                    | `variance.rs` |
| B3        | `vec_no_coerce_to_different_inner_type` + `vec_no_coerce_widening_inner` + `vec_no_coerce_through_inheritance` | `variance.rs` |
| B4        | `vec_same_type_works`                                 | `variance.rs` |
| B5        | `option_covariant_through_mut_to_immut_ref` + `option_covariant_through_integer_widening` + `option_no_coerce_when_inner_incompatible` | `variance.rs` |
| B6        | `variance_module_is_wired_into_typeck`                | `variance.rs` |

---

## Out of scope (v2)

- User-controllable variance annotations (`#[covariant]` /
  `#[contravariant]`).
- Variance for higher-kinded types and GATs.
- Contravariance — Riven has no surface today that admits it
  (closures are invariant in their parameter types).
