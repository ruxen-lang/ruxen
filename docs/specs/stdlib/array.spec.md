# Spec — `Array[T]`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.1](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 2 #04; method surface self-hosted in
`library/std/src/array.rvn` since #06.8 T#14.

`Array[T]` is a growable contiguous heap array with `O(1)` amortised
push/pop and `O(1)` index access.  The C runtime stores
`{ ptr, len, cap }` and grows by doubling.

---

## B1 — `Array.from(v: T)` is currently lenient at typeck

The constructor `Array.from(...)` accepts any single value at typecheck
time today (v1 simplification — strict `From` impls deferred to v2).
A v2 cleanup will tighten this so that only `Array.from(iter)` etc. are
accepted.  Until then, `Array.from(1)` typechecks (and the runtime will
do its best at MIR-lowering time).

## B2 — `pop() -> Option[T]`

Removes and returns the last element, or `nil` when empty.

## B3 — Indexing yields the element type

**Given** `v: Array[Int]`
**Then** `v[0]` has type `Int` (not `Option[Int]` — bounds checking
is dynamic; out-of-bounds panics).

## B4 — `with_capacity(n)` accepts non-Int args today

Like B1, `Array.with_capacity(...)` is currently lenient — passing a
`String` typechecks (the runtime coerces).  Tightening to `Int`-only
is a v2 cleanup.

## B5 — `dedup()` typechecks as `Unit`

In-place dedup of consecutive equal elements.  Returns nothing.

## B6 — `retain(|x| pred)` closure typechecks

Filters in-place; the closure receives `&T` and returns `Bool`.

## B7 — `sort_by(|a, b| order)` closure typechecks

Stable sort in place.  The closure receives `&T, &T` and returns
`Int` (Ordering — negative / zero / positive).

## B8 — `Array.from_iter(iter)` typechecks (static method)

Constructs an `Array[T]` by draining the iterator.

## B9 — Equality (`==`) yields `Bool`

Pairwise comparison of two arrays at corresponding indices.

---

## Pin tests

| Behaviour | Test fn                                                | File                       |
|-----------|--------------------------------------------------------|----------------------------|
| B1        | `vec_from_int_is_currently_accepted_at_typeck`         | `stdlib_array_negatives.rs`  |
| B2        | `vec_pop_returns_option_typechecks`                    | `stdlib_array_negatives.rs`  |
| B3        | `vec_index_yields_element_type`                        | `stdlib_array_negatives.rs`  |
| B4        | `vec_with_capacity_string_arg_is_currently_accepted_at_typeck` | `stdlib_array_negatives.rs` |
| B5        | `vec_dedup_typechecks_as_unit`                         | `stdlib_array_negatives.rs`  |
| B6        | `vec_retain_closure_typechecks`                        | `stdlib_array_negatives.rs`  |
| B7        | `vec_sort_by_closure_typechecks`                       | `stdlib_array_negatives.rs`  |
| B8        | `vec_from_iter_static_typechecks`                      | `stdlib_array_negatives.rs`  |
| B9        | `vec_equality_yields_bool`                             | `stdlib_array_negatives.rs`  |

Runtime round-trips covered by E2E fixtures
`tests/release-e2e/cases/107_array_push_pop.rvn` and the `60x_iter_*`
series.

---

## Gaps + v2 cleanups

- B1, B4: tighten `Array.from(...)` and `Array.with_capacity(...)` to
  reject obviously wrong arg types.
- `Array.first / last / contains / clone / reverse` now have direct
  runtime pin tests in `stdlib_array_runtime.rs` (added 2026-05):
  `vec_first_returns_first_element`, `vec_last_returns_last_element`,
  `vec_contains_finds_element`, `vec_clone_returns_independent_copy`,
  `vec_reverse_inverts_order`.

## Out of scope (v2)

- `Deque` / `LinkedList`.
- `slice` primitives independent of `Array`.
- `drain`, `splice`, `chunks`, `windows` — wired piecemeal as
  needed.
