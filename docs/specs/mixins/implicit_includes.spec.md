# Spec — Implicit includes (structural mixins)

**Source docs:**
[docs/requirements/tier1_05_implicit_includes.md](../../requirements/tier1_05_implicit_includes.md),
canonical surface-syntax §3.6
([ruby-naming.spec.md](../syntax/ruby-naming.spec.md)).

**Status:** shipped Phase 2 #C1-#C2 (Debug for struct + enum) + extended
implicit includes Phase 2 #C3-#C4 (Clone, PartialEq, Eq, Hashable,
Default, Ord, PartialOrd, Copy).

For the fixed set of **structural mixins** listed in §3.6 of the
canonical surface-syntax spec, the compiler treats the `include`
directive as implicit when the type's fields structurally support
the mixin.  The user may write `include Debug, Clone, Eq, Hashable`
explicitly as the "loud form" — implicit and explicit lower to the
same compiler-generated method body.

`Send` and `Sync` are auto-mixins handled by the same field-rule;
they are never written explicitly with `include` in ordinary code.
Users opt out via `exclude Send` / `exclude Sync` in the body, or
opt in for inference-incompatible structures via
`unsafe include Send` / `unsafe include Sync` (the only legal use
of `unsafe include`).

---

## B1 — Implicit `Debug` on a struct prints named-field shape

**Given** `struct Point` with fields `x: Int, y: Int` (no explicit
`include Debug`)
**When** the program evaluates `"#{p:?}"` for `p = Point { x: 1, y: 2 }`
**Then** the result is `"Point { x: 1, y: 2 }"`.

## B2 — Implicit `Debug` on a unit-variant enum prints variant name

**Given** `enum Color` with variants `Red`, `Green`, `Blue` and
`let c = Color.Red`
**Then** `"#{c:?}"` yields `"Red"`.

## B3 — Implicit `Debug` on an enum named-field variant prints braces

**Given** an enum variant `Move { x: Int, y: Int }`
**Then** the Debug output is `"Move { x: 1, y: 2 }"`.

## B4 — Explicit `:?` spec dispatches through Debug

**Given** any type whose fields support `Debug` (implicit) and a
format spec with `?`
**Then** `lower_interpolation` routes through `{Name}_to_debug`
(rather than the Display path).

## B5 — Implicit `Clone` synthesises `clone() -> Self`

Both primitive-only structs and structs containing `String` /
nested-Clone fields produce a deep copy.  Class types and enums
work identically — when every field is `Clone`, the compiler
synthesises a `clone` body.

## B6 — Implicit `PartialEq` synthesises field-wise `==`

Two values of the same struct / enum type compare equal iff every
field / payload is pairwise `==`.

## B7 — Implicit `Eq` requires `PartialEq` on every field

Marker mixin — no synthesised methods, but typeck rejects an
explicit `include Eq` if any field type does not also satisfy `Eq`,
and an implicit `Eq` is suppressed when the rule fails.

## B8 — Implicit `Hashable` synthesises a hash body

Allows the type to be used as a `Map` / `Set` key.  FNV mixer over
fields in source order.

## B9 — Implicit `Default` synthesises `Type.default()` class method

Each field is initialised to its type's `Default`.  Empty enums
are rejected (`E0616`) when an explicit `include Default` is
written; implicit `Default` is simply not synthesised for an
empty enum.

## B10 — Implicit `Ord` and `PartialOrd` synthesise comparators

Field-wise lexicographic ordering in source declaration order.

## B11 — Implicit `Copy` marker

A `struct` whose every field is `Copy` implicitly includes `Copy`,
making the type bitwise-copyable; the borrow checker stops emitting
move-out diagnostics.  A `class` never implicitly includes `Copy`
(reference semantics).

## B12 — Explicit-include validation negative diagnostics

The validation diagnostics fire at the include site for the explicit
("loud form") `include D1, D2` directive.  With implicit-only, the
equivalent diagnostic fires at the use site (later, but still caught
at compile time).

| Code   | Rejection                                                |
|--------|----------------------------------------------------------|
| E0607  | `include` on an invalid target (e.g. a `def`, `use`)     |
| E0610  | `include Clone` on struct with non-Clone field           |
| E0611  | `include Clone` on enum with non-Clone payload           |
| E0613  | `include PartialEq` on struct with non-Eq field          |
| E0615  | `include Hashable` on struct with non-Hashable field     |
| E0616  | `include Default` on empty enum                          |
| E0617  | `include Ord` on struct with non-Ord field               |
| E0618  | `include PartialOrd` on struct with non-PartialOrd field |

## B13 — Generated impls dispatch through mixin bounds

Generic code constrained by `T: Ord` / `T: Hashable` / `T: Clone` /
`T: PartialEq` etc. resolves correctly when called with a type
that picks up the mixin implicitly.

## B14 — Implicit `Default` emits a concrete class-level method

`Type.default()` is callable directly without `any`-mixin machinery
(matches Rust's "associated function" emission).

## B15 — User override wins over implicit body

If the class body defines a method whose name matches an
implicit-include's synthesised body (e.g. a user-written
`def to_debug -> String`), the user definition wins; the implicit
include does not provide a duplicate body.

---

## Pin tests

| Behaviour | Test fn                                       | File                                     |
|-----------|-----------------------------------------------|------------------------------------------|
| B1        | `struct_with_derive_debug_prints_named_fields`| `implicit_debug_formats_struct.rs`         |
| B2        | `enum_with_derive_debug_unit_variant_prints_name` | `implicit_debug_formats_enum.rs`       |
| B3        | `enum_with_derive_debug_named_field_variant_prints_braces` | `implicit_debug_formats_enum.rs` |
| B4        | `enum_with_derive_debug_explicit_debug_spec_dispatches` | `implicit_debug_formats_enum.rs`  |
| B5        | E2E `202_implicit_clone_struct_primitive.rx` + 203/204/205 | `tests/release-e2e/cases/` |
| B6        | `derive_partial_eq_compares_fields_struct`    | `implicit_partial_eq_returns_correct.rs`   |
| B6        | E2E `206_implicit_partial_eq.rx` + `133_implicit_partial_eq.rx` | `tests/release-e2e/cases/` |
| B8        | `derive_hashable_dispatches_through_trait_bounds` | `implicit_mixin_dispatch.rs`           |
| B8        | E2E `207_implicit_hash.rx` + `132_implicit_hashable.rx` | `tests/release-e2e/cases/`   |
| B9, B14   | `derive_default_emits_concrete_static_method` | `implicit_mixin_dispatch.rs`               |
| B9        | E2E `208_implicit_default.rx`                | `tests/release-e2e/cases/`               |
| B10       | `derive_ord_and_partial_ord_dispatch_through_trait_bounds` | `implicit_mixin_dispatch.rs` |
| B10       | E2E `126_implicit_ord_compare.rx` + `209_implicit_ord.rx` | `tests/release-e2e/cases/` |
| B11       | E2E `131_implicit_copy.rx` + `derive_copy_clone_fixture_typechecks_cleanly` | `implicit_diagnostics.rs` |
| B12 E0607 | `derive_invalid_target_reports_e0607`         | `implicit_diagnostics.rs`                  |
| B12 E0610/E0611 | `derive_clone_on_struct_with_non_clone_field_emits_e0610_or_e0611` + enum equivalent | `implicit_negatives.rs` |
| B12 E0613 | `derive_partial_eq_on_struct_with_non_eq_field_emits_e0613` | `implicit_negatives.rs`        |
| B12 E0615 | `derive_hash_on_struct_with_non_hash_field_emits_e0615` | `implicit_negatives.rs`            |
| B12 E0616 | `derive_default_on_empty_enum_emits_e0616`    | `implicit_negatives.rs`                    |
| B12 E0617 | `derive_ord_on_struct_with_non_ord_field_emits_e0617` | `implicit_negatives.rs`              |
| B12 E0618 | `derive_partial_ord_on_struct_with_non_partial_ord_field_emits_e0618` | `implicit_negatives.rs` |
| B12 catalog | `derive_validation_reports_expected_codes`  | `implicit_diagnostics.rs`                  |
| B13       | covered transitively by B8/B9/B10 + E2E fixtures |                                        |

Error-code registry is itself pin-tested by
`error_code_registry.rs::e0610_through_e0618_derive_codes_are_registered`.

<!-- TODO(migration): pin-test fn names still mention `derive_*` and the error-code registry constant names still mention `derive_codes`. These are internal Rust identifiers; rename when the spec author confirms it is in scope. -->

---

## Out of scope (v2)

- User-extensible implicit-include rules.  v1 has the fixed
  allow-list of structural mixins (§3.6).
- Implicit `Display` (always user-written for now).
- Multi-line / pretty Debug output (`"#{value:#?}"` syntax).
