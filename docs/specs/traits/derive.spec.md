# Spec — `derive <Trait>` (compiler-generated impls)

**Source docs:**
[docs/requirements/tier1_05_derive_macros.md](../../requirements/tier1_05_derive_macros.md).

**Status:** shipped Phase 2 #C1-#C2 (Debug for struct + enum) + extended
derives Phase 2 #C3-#C4 (Clone, PartialEq, Eq, Hash, Default, Ord,
PartialOrd, Copy).

`derive <Trait>` on a `struct`, `class`, or `enum` synthesises a
compiler-generated trait implementation that mirrors the type's field
or variant shape.

---

## B1 — `derive Debug` on a struct prints named-field shape

**Given** `struct Point` with fields `x: Int, y: Int` and `derive Debug`
**When** the program evaluates `"#{p:?}"` for `p = Point { x: 1, y: 2 }`
**Then** the result is `"Point { x: 1, y: 2 }"`.

## B2 — `derive Debug` on a unit-variant enum prints variant name

**Given** `enum Color { Red, Green, Blue }` with `derive Debug` and
`let c = Color::Red`
**Then** `"#{c:?}"` yields `"Red"`.

## B3 — `derive Debug` on an enum named-field variant prints braces

**Given** an enum variant `Move { x: Int, y: Int }` with `derive Debug`
**Then** the Debug output is `"Move { x: 1, y: 2 }"`.

## B4 — Explicit `:?` spec dispatches through Debug

**Given** any `derive Debug` type and a spec with `?`
**Then** `lower_interpolation` routes through `{Name}_to_debug`
(rather than the Display path).

## B5 — `derive Clone` synthesises `clone() -> Self`

Both primitive-only structs and structs containing `String` /
nested-derive-Clone fields produce a deep copy.  Class types and
enums work identically.

## B6 — `derive PartialEq` synthesises field-wise `==`

Two values of the same struct / enum type compare equal iff every
field / payload is pairwise `==`.

## B7 — `derive Eq` requires `PartialEq` on every field

Marker trait — no synthesised methods, but typeck rejects an `Eq`
derive if any field type does not also satisfy `Eq`.

## B8 — `derive Hash` synthesises a `Hash` impl

Allows the type to be used as `HashMap` / `HashSet` key.

## B9 — `derive Default` synthesises `T::default()` static method

Each field is initialised to its type's `Default`.  Empty enums
are rejected (`E0616`).

## B10 — `derive Ord` and `derive PartialOrd` synthesise comparators

Field-wise lexicographic ordering matching Rust's derive semantics.

## B11 — `derive Copy` marker

`derive Copy` (with `Clone`) makes the type bitwise-copyable; the
borrow checker stops emitting move-out diagnostics.

## B12 — Derive-validation negative diagnostics

| Code   | Rejection                                              |
|--------|--------------------------------------------------------|
| E0607  | `derive` on an invalid target (e.g. a `def`, `use`)    |
| E0610  | `derive Clone` on struct with non-Clone field          |
| E0611  | `derive Clone` on enum with non-Clone payload          |
| E0613  | `derive PartialEq` on struct with non-Eq field         |
| E0615  | `derive Hash` on struct with non-Hash field            |
| E0616  | `derive Default` on empty enum                         |
| E0617  | `derive Ord` on struct with non-Ord field              |
| E0618  | `derive PartialOrd` on struct with non-PartialOrd field |

## B13 — Generated impls dispatch through trait bounds

Generic code constrained by `T: Ord` / `T: Hash` / `T: Clone` /
`T: PartialEq` etc. resolves correctly when called with a
derive-generated type.

## B14 — `derive Default` emits a concrete static method

`T::default()` is callable directly without trait-object machinery
(matches Rust's "associated function" emission).

---

## Pin tests

| Behaviour | Test fn                                       | File                                     |
|-----------|-----------------------------------------------|------------------------------------------|
| B1        | `struct_with_derive_debug_prints_named_fields`| `derive_debug_formats_struct.rs`         |
| B2        | `enum_with_derive_debug_unit_variant_prints_name` | `derive_debug_formats_enum.rs`       |
| B3        | `enum_with_derive_debug_named_field_variant_prints_braces` | `derive_debug_formats_enum.rs` |
| B4        | `enum_with_derive_debug_explicit_debug_spec_dispatches` | `derive_debug_formats_enum.rs`  |
| B5        | E2E `202_derive_clone_struct_primitive.rvn` + 203/204/205 | `tests/release-e2e/cases/`   |
| B6        | `derive_partial_eq_compares_fields_struct`    | `derive_partial_eq_returns_correct.rs`   |
| B6        | E2E `206_derive_partial_eq.rvn` + `133_derive_partial_eq.rvn` | `tests/release-e2e/cases/` |
| B8        | `derive_hashable_dispatches_through_trait_bounds` | `derive_trait_dispatch.rs`           |
| B8        | E2E `207_derive_hash.rvn` + `132_derive_hashable.rvn` | `tests/release-e2e/cases/`       |
| B9, B14   | `derive_default_emits_concrete_static_method` | `derive_trait_dispatch.rs`               |
| B9        | E2E `208_derive_default.rvn`                  | `tests/release-e2e/cases/`               |
| B10       | `derive_ord_and_partial_ord_dispatch_through_trait_bounds` | `derive_trait_dispatch.rs` |
| B10       | E2E `126_derive_ord_compare.rvn` + `209_derive_ord.rvn` | `tests/release-e2e/cases/`     |
| B11       | E2E `131_attr_derive_copy.rvn` + `derive_copy_clone_fixture_typechecks_cleanly` | `derive_diagnostics.rs` |
| B12 E0607 | `derive_invalid_target_reports_e0607`         | `derive_diagnostics.rs`                  |
| B12 E0610/E0611 | `derive_clone_on_struct_with_non_clone_field_emits_e0610_or_e0611` + enum equivalent | `derive_negatives.rs` |
| B12 E0613 | `derive_partial_eq_on_struct_with_non_eq_field_emits_e0613` | `derive_negatives.rs`        |
| B12 E0615 | `derive_hash_on_struct_with_non_hash_field_emits_e0615` | `derive_negatives.rs`            |
| B12 E0616 | `derive_default_on_empty_enum_emits_e0616`    | `derive_negatives.rs`                    |
| B12 E0617 | `derive_ord_on_struct_with_non_ord_field_emits_e0617` | `derive_negatives.rs`              |
| B12 E0618 | `derive_partial_ord_on_struct_with_non_partial_ord_field_emits_e0618` | `derive_negatives.rs` |
| B12 catalog | `derive_validation_reports_expected_codes`  | `derive_diagnostics.rs`                  |
| B13       | covered transitively by B8/B9/B10 + E2E fixtures |                                        |

Error-code registry is itself pin-tested by
`error_code_registry.rs::e0610_through_e0618_derive_codes_are_registered`.

---

## Out of scope (v2)

- User-extensible `derive` macros (`derive(MyTrait)`).  v1 has a
  fixed allow-list of built-in derives.
- `derive Display` (always user-written for now).
- Multi-line / pretty Debug output (`{:#?}` syntax).
