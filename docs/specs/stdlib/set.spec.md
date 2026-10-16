# Spec — `Set[T]`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.3](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 2 #04-#05; method surface self-hosted in
`library/std/set/src/lib.rx` since #06.8 T#16.

`Set[T]` is a thin layer over `Hash[T, nil]` — same hashing
semantics, same `Hashable` bound on `T`.

---

## B1 — `Set[T]` is the canonical name

`Set.new()` and `[...].to_set` (the `{...}` literal is
reserved for `Hash` per §3.22) construct the same runtime type.

## B2 — `insert(x)` typechecks as returning `nil` (v1 simplification)

The runtime returns whether the element was newly inserted, but the
typeck surface reports `nil` today (v2 will surface the `Bool`).

## B3 — `remove(x) -> Bool`

Removes the element if present and returns `true`; returns `false`
otherwise.

## B4 — Set operations return fresh sets

`union(other)`, `intersection(other)`, `difference(other)`, and
`symmetric_difference(other)` each return a new `Set[T]` —
operands are not modified.

## B5 — `to_a` returns an `Array[&T]`

`to_a` materialises the set into an `Array[&T]`.  The runtime
guarantees no duplicates; order is unspecified.

## B6 — Equality (`==`) yields `Bool`

`set1 == set2` returns `true` iff the two sets contain the same
elements (order-independent).

## B7 — Element type must be `Hashable`

**Given** `Set[Hash[K, V]]` (set of hashes — hashes are not
hashable)
**Then** typeck emits diagnostic `E0615`.

---

## Pin tests

| Behaviour | Test fn                                       | File                          |
|-----------|-----------------------------------------------|-------------------------------|
| B1, B2    | "ignores a duplicate insert"                  | `library/std/set/tests/set_test.rx` |
| B3        | "reports whether remove found the item"       | `library/std/set/tests/set_test.rx` |
| B4        | "combines sets with union, intersection and difference" | `library/std/set/tests/set_test.rx` |
| B5        | "to_set from an array and to_a back"          | `library/std/set/tests/set_test.rx` |
| B6        | "== ignores insertion order"                  | `library/std/set/tests/set_test.rx` |
| B7        | `hashset_with_non_hash_element_emits_e0615` + `hashset_of_hashmap_emits_e0615` | `stdlib_map_negatives.rs` |

The `.rx` tests run with `scripts/test-stdlib.sh set`.

---

## Out of scope (v2)

- A sorted variant.
- Surfacing `insert`'s "newly inserted" bool through typeck.
- Enumerating without materialising an `Array` first.
