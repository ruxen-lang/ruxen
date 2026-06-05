# Spec — `std.iter` (Iterator mixin) — RETIRED

> **RETIRED by the Ruby-surface migration.** The iterator layer no
> longer exists: there is no `.iter` / `.into_iter` / `.collect` /
> `from_iter`, and the `Iterator` / `FromIterator` mixins are removed
> (`library/std/iter/` is deleted and dropped from the bootstrap). The
> block combinators (`each` / `map` / `select` / `reject` / `reduce` /
> `find` / `index` / `all?` / `any?` / `each_with_index` / …) are
> methods **directly on `Array`** (and `Hash` / `Set`), inlined at the
> MIR layer — collections are Ruby `Enumerable`-shaped. See
> `docs/specs/syntax/ruby-naming.spec.md` §3.23. The historical
> `*Iter` design below is kept only as a record of the v1 shortcut.

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.8](../../requirements/tier1_01_stdlib.md).

**Status:** RETIRED (was: shipped Phase 2 #05 as a typeck surface with
a runtime pass-through to `Array` helpers).

`.iter()` on an `Array[T]` returns a `*Iter` class.  Ruxen v1 takes a
pragmatic shortcut: the `Iter` is a runtime no-op (`ruxen_iter_to_vec`)
and every method routes back to a `ruxen_vec_*` helper.  This means
the iterator surface composes correctly at the type level and lowers
to efficient code, but it materialises the whole collection eagerly
rather than streaming.  Specs below describe the **observable**
behaviour from the program's perspective; the eagerness is an
implementation detail covered in tier1_01_stdlib §5.8.

---

## Pipeline behaviours

The following method calls are typeable and produce the expected
result types.  Each pin is a "compiles + types check" test, not a
runtime round-trip (the runtime layer reuses the audited
`ruxen_vec_*` helpers).

| B# | Pipeline                                              | Result type     |
|----|-------------------------------------------------------|-----------------|
| B1 | `arr.reduce(init) { |acc, x| acc + x }`              | accumulator type|
| B2 | `arr.all? { |x| pred(x) }`                            | `Bool`          |
| B3 | `arr.any? { |x| pred(x) }`                            | `Bool`          |
| B4 | `arr.take(n)` / `.drop(n)`                            | `Array[T]`      |
| B5 | `.take(n).sum()` / `.drop(n).sum()`                   | element type    |
| B6 | `.each_with_index` passthrough                        | `Array[(USize, T)]` |
| B7 | `.select(pred).count()`                              | `USize` (Int)   |
| B8 | `.map(f)`                                             | `Array[U]`      |
| B9 | `.sum()` / `.count()` on `Array[Int]`                | `Int`           |
| B10| `.drop(a).take(b).count()`                           | `USize`         |
| B11| `.take(n).reduce(init, f)`                            | accumulator     |
| B12| `.drop(n).all?(pred)`                                 | `Bool`          |
| B13| `.chain(other)` / `.chain(other).sum()`               | `Array[T]` / `T`|
| B14| `.zip(other).count()`                                 | `USize`         |
| B15| `.map(f)` / `.join("")` / `.to_h` / `.to_set`         | named collection |
| B16| `v.join("")`, `pairs.to_h`, `v.to_set`               | named collection |

## Negative behaviours (rejection)

| B# | Mis-use                                              | Diagnostic       |
|----|------------------------------------------------------|------------------|
| B17| `sum()` on `Array[String]`                          | typeck error     |
| B18| `.to_h` on a non-pair array                          | typeck error     |
| B19| `sum()` on `Array[Int]` still passes (tightening did not break the positive case) | none |

---

## Pin tests

| Behaviour | Test fn                                  | File                  |
|-----------|------------------------------------------|-----------------------|
| B1        | `iter_fold_compiles_with_int_accumulator`| `stdlib_iterator.rs`  |
| B2        | `iter_all_compiles_returning_bool`       | `stdlib_iterator.rs`  |
| B3        | `iter_any_compiles_returning_bool`       | `stdlib_iterator.rs`  |
| B4        | `iter_take_compiles` + `iter_skip_compiles` | `stdlib_iterator.rs` |
| B5        | `iter_take_then_sum_compiles` + `iter_skip_then_sum_compiles` | `stdlib_iterator.rs` |
| B6        | `iter_enumerate_passthrough_compiles`    | `stdlib_iterator.rs`  |
| B7        | `iter_filter_then_count_compiles`        | `stdlib_iterator.rs`  |
| B8        | `iter_map_changes_item_type_then_collect_vec_compiles` | `stdlib_iterator.rs` |
| B9        | `iter_sum_still_compiles` + `iter_count_still_compiles` | `stdlib_iterator.rs` |
| B10       | `iter_skip_then_take_then_count_compiles`| `stdlib_iterator.rs`  |
| B11       | `iter_take_then_fold_compiles`           | `stdlib_iterator.rs`  |
| B12       | `iter_skip_then_all_compiles`            | `stdlib_iterator.rs`  |
| B13       | `iter_chain_compiles` + `iter_chain_then_sum_compiles` | `stdlib_iterator.rs` |
| B14       | `iter_zip_then_count_compiles`           | `stdlib_iterator.rs`  |
| B15       | `iter_collect_vec_compiles` + `iter_collect_string_compiles` + `iter_collect_hashmap_compiles` + `iter_collect_hashset_compiles` | `stdlib_iterator.rs` |
| B16       | `string_from_iter_compiles` + `hashmap_from_iter_compiles` + `hashset_from_iter_compiles` | `stdlib_iterator.rs` |
| B17       | `sum_on_string_iter_typeck_rejects`      | `stdlib_iterator.rs`  |
| B18       | `collect_hashmap_rejects_non_pair_items` | `stdlib_iterator.rs`  |
| B19       | `sum_on_int_iter_still_compiles_after_tightening` | `stdlib_iterator.rs` |

Runtime correctness rides on the `ruxen_vec_*` helpers, which have
their own pin tests + E2E fixtures (`tests/release-e2e/cases/600_…`
through `611_…`).

---

## Out of scope (v2)

- True streaming iterators (don't materialise the source eagerly).
- `Iterator` as a user-implementable mixin.
- `iter_var` / `into_iter` distinctions (v1 is `iter` only).
- `Peekable`, `Cycle`, `StepBy`, `Inspect`, `Scan`, `FlatMap`,
  `Flatten` — only the methods listed above are wired.
