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

## B6 — `select! { |x| pred }` block typechecks

Filters in-place; the block receives `&T` and returns `Bool`. [implemented];
The old Rust name `retain` was renamed to `select!` and is [retired].

## B7 — `sort_by { |a, b| order }` block typechecks

[implemented]. Stable sort in place.  The block receives `&T, &T` and returns
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

## B11 — Block combinators (Enumerable shape) [implemented]

There is no iterator layer (naming spec section 18, rule 3): the block
methods are called directly on an `Array`, most of them `Enumerable`
defaults in `library/std/core/src/lib.rx`. Each step builds a new `Array`
eagerly. These behaviours moved here from the retired `iterator.spec.md`.

| Method | Result | Status |
|---|---|---|
| `xs.reduce(init) { \|acc, x\| acc + x }` | the accumulator type | [implemented] |
| `xs.all? { \|x\| pred }`, `xs.any? { \|x\| pred }` | `Bool` | [implemented] |
| `xs.each_with_index { \|x, i\| ... }` | `nil`; the block gets the element, then its `Int` index | [implemented] |
| `xs.chain(&other)` | a new `Array[T]`, `xs` then `other` | [implemented] |
| `xs.zip(other)` | `Array[(T, U)]`, one pair per position | [implemented] |
| `xs.sum` | the element type, for an element type that includes `Add` | [implemented]; `String` elements are rejected at typeck |
| `xs.count` | the number of elements | [implemented] |
| `xs.select { ... }.count`, `xs.take(n).sum`, `xs.drop(a).take(b).count` | chains of the above | [implemented] |

Works today [implemented]:

```ruby
xs = [1, 2, 3]
total = xs.reduce(0) { |acc, x| acc + x }
puts total                                  # 6
all_pos = xs.all? { |x| x > 0 }
puts all_pos                                # true
has_two = xs.any? { |x| x == 2 }
puts has_two                                # true
xs.each_with_index { |x, i| puts "#{i}:#{x}" }   # 0:1, 1:2, 2:3
both = xs.chain(&[4])
puts both.sum                               # 10
pairs = xs.zip(["a", "b", "c"])
puts pairs.size                             # 3
puts xs.sum                                 # 6
puts xs.count                               # 3
```

## Owner decisions (2026-10-25)

- D1 a: `Array#[]` and `#[]=` become ordinary `.rx` lib declarations and
  indexing an ordinary method call, with `[]=` through a declared mutable
  place accessor [planned](../../../TODO.md#t-d1). Today MIR still emits C names for
  indexing.
- D7 b: `join`, `min`, `max`, `uniq`, `include?` and `sum` become
  `Enumerable` defaults, so `Set` and `Hash` get them. They keep the method
  `where` bound that works today (`def sum -> T where T: Add`), which is
  [deprecated] in favour of the RBS bound `[T < Bound]`
  [planned](../../../TODO.md#t-d7-respell); OQ-6 stays open [planned](../../../TODO.md#t-d7).
- D13 a: `get_mut` becomes `alias get_mut get_var` over a true slot accessor,
  and E0722 is strict again [planned](../../../TODO.md#t-d13). Today `get`, `get_mut` and
  `get_var` share one helper that boxes a copy, so do not rely on a write
  through `get_mut` reaching the array.
- D15 a: `sum` becomes Ruby's `sum(init)` plus a no-argument form for a
  numeric element type [planned](../../../TODO.md#t-d15); today only the no-argument
  `sum` exists.

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
| B11       | "sum, count and reduce, including on an empty array"; "all?, any?, none? and one?"; "each and each_with_index visit in order"; "chain and zip"; `sum_on_string_iter_typeck_rejects` | `library/std/iter/tests/aggregate_test.rx`, `library/std/iter/tests/transform_test.rx`, `compiler/ruxen_core/tests/it/stdlib_iterator.rs` |

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
