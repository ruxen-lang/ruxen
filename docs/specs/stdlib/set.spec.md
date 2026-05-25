# Spec — `Set[T]`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.3](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 2 #04-#05; method surface self-hosted in
`library/std/src/set.rx` since #06.8 T#16.

`Set[T]` is a thin layer over `Map[T, ()]` — same hashing
semantics, same `Hashable` bound on `T`.

---

## B1 — `Set[T]` is the canonical name

`Set.new()` and `Set.from_iter([...])` (the `{...}` literal is
reserved for `Map` per §3.22) construct the same runtime type.

## B2 — `insert(x)` typechecks as `Unit` (v1 simplification)

The runtime returns whether the element was newly inserted, but the
typeck surface reports `Unit` today (v2 will surface the `Bool`).

## B3 — `remove(x) -> Bool`

Removes the element if present and returns `true`; returns `false`
otherwise.

## B4 — Set operations return fresh sets

`union(other)`, `intersection(other)`, `difference(other)`, and
`symmetric_difference(other)` each return a new `Set[T]` —
operands are not modified.

## B5 — `iter()` returns an `Array[T]`

v1 simplification: `iter()` materialises the set into an `Array[T]`.  The
runtime guarantees no duplicates; order is unspecified.

## B6 — Equality (`==`) yields `Bool`

`set1 == set2` returns `true` iff the two sets contain the same
elements (order-independent).

## B7 — Element type must be `Hashable`

**Given** `Set[Map[K, V]]` (set of maps — maps are not
hashable)
**Then** typeck emits diagnostic `E0615`.

---

## Pin tests

| Behaviour | Test fn                                       | File                          |
|-----------|-----------------------------------------------|-------------------------------|
| B1        | `hashset_alias_constructs_via_either_name`    | `stdlib_set.rs`           |
| B2        | `hashset_insert_typechecks_as_unit_today`     | `stdlib_set.rs`           |
| B3        | `hashset_remove_returns_bool`                 | `stdlib_set.rs`           |
| B4        | `hashset_set_ops_return_fresh_set`            | `stdlib_set.rs`           |
| B5        | `hashset_iter_returns_vec`                    | `stdlib_set.rs`           |
| B6        | `hashset_equality_yields_bool`                | `stdlib_set.rs`           |
| B7        | `hashset_with_non_hash_element_emits_e0615` + `hashset_of_hashmap_emits_e0615` | `stdlib_map_negatives.rs` |

Runtime round-trips covered by E2E fixture `105_set_basic.rx` and
`611_iter_collect_set.rx`.

---

## Out of scope (v2)

- `BTreeSet` / ordered variant.
- Surfacing `insert`'s "newly inserted" bool through typeck.
- True streaming `iter()` that doesn't materialise an `Array`.
