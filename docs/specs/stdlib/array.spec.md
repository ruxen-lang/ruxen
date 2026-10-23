# Spec — `Array[T]`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.1](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 2 #04; method surface self-hosted in
`library/std/array/src/lib.rx` since #06.8 T#14.

`Array[T]` is a growable contiguous heap array with `O(1)` amortised
push/pop and `O(1)` index access.  The C runtime stores
`{ ptr, len, cap }` and grows by doubling.

---

## B1 — `Array.from(v: T)` is currently lenient at typeck

The constructor `Array.from(...)` accepts any single value at typecheck
time today (v1 simplification — strict conversion checks deferred to v2).
A v2 cleanup will tighten this so that only collection arguments are
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

## B5 — `dedup()` returns `nil`

In-place dedup of consecutive equal elements.  Returns `nil`.

## B6 — `retain(|x| pred)` closure typechecks

Filters in-place; the closure receives `&T` and returns `Bool`.

## B7 — `sort_by(|a, b| order)` closure typechecks

Stable sort in place.  The closure receives `&T, &T` and returns
`Int` (Ordering — negative / zero / positive).

## B8 — `Array.from_iter(iter)` — REMOVED

Retired by the Ruby-surface migration (no iterator layer). Build a new
collection with the Ruby `to_*` conversions instead: `[…].to_set`,
`pairs.to_h`, `chars.join("")`. An `Array` is built from a literal
`[…]` or grown with `.push`.

## B9 — Equality (`==`) yields `Bool`

Pairwise comparison of two arrays at corresponding indices.

## B10 — `sort` returns a new array; `sort!` sorts in place

Ruby parity. `sort` returns a NEW `Array[T]` in ascending order by the
element type's own `<` (stable) and leaves the receiver unchanged.
`sort!` sorts the receiver in place and returns `nil`.

---

## Pin tests

| Behaviour | Test fn                                                | File                       |
|-----------|--------------------------------------------------------|----------------------------|
| B1        | _none; `Array.from(123)` is accepted at typeck (TODO.md Known issues)_ | — |
| B2        | "push and pop, with pop on empty returning nil"        | `library/std/array/tests/mutate_test.rx` |
| B3        | "indexes by position"                                  | `library/std/array/tests/access_test.rx` |
| B4        | "with_capacity reserves room"                          | `library/std/array/tests/mutate_test.rx` |
| B5        | "dedup drops consecutive duplicates only"              | `library/std/array/tests/mutate_test.rx` |
| B6        | "select! keeps matching elements"                      | `library/std/array/tests/mutate_test.rx` |
| B7        | "sort_by takes a comparator"                           | `library/std/array/tests/sort_test.rx` |
| B8        | _removed — `from_iter` retired_                        | —                            |
| B9        | "== compares element-wise"                             | `library/std/array/tests/access_test.rx` |
| B10       | "sort returns a new array and sort! sorts in place", `vec_sort_bang_returns_nil` | `library/std/array/tests/sort_test.rx`, `stdlib_array_negatives.rs` |

The `.rx` tests run with `scripts/test-stdlib.sh array`.

---

## Gaps + v2 cleanups

- B1, B4: tighten `Array.from(...)` and `Array.with_capacity(...)` to
  reject obviously wrong arg types.
- `Array.first / last / include? / clone / reverse` are tested in
  `library/std/array/tests/access_test.rx` and `mutate_test.rx`.

## Out of scope (v2)

- `Deque` / `LinkedList`.
- `slice` primitives independent of `Array`.
- `drain`, `splice`, `chunks`, `windows` — wired piecemeal as
  needed.
