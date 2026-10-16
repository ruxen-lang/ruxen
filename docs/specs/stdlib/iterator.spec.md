# Spec — `std.iter` (Iterator mixin) — RETIRED

> **RETIRED by the Ruby-surface migration.** The iterator layer no
> longer exists: there is no `.iter` / `.into_iter` / `.collect` /
> `from_iter`, and the `Iterator` / `FromIterator` mixins are removed
> (`library/std/iter/` is deleted and dropped from the bootstrap). The
> block combinators (`each` / `map` / `select` / `reject` / `reduce` /
> `find` / `index` / `all?` / `any?` / `each_with_index` / …) are
> methods **directly on `Array`** (and `Hash` / `Set`), inlined at the
> MIR layer — collections are Ruby `Enumerable`-shaped. See
> `docs/specs/syntax/ruby-naming.spec.md` §18 (rule 3). The behaviour table
> below now describes those `Array` methods; the pin-test names keep
> their historical `iter_*` prefix.

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.8](../../requirements/tier1_01_stdlib.md).

**Status:** RETIRED (was: shipped Phase 2 #05 as a typeck surface with
a runtime pass-through to `Array` helpers).

Every block method below is called directly on an `Array[T]` and
routes to a `ruxen_vec_*` helper.  Each step materialises a new
`Array` eagerly rather than streaming.  Specs below describe the
**observable** behaviour from the program's perspective.

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
| B1, B9, B11 | "sum, count and reduce, including on an empty array" | `library/std/iter/tests/aggregate_test.rx` |
| B2, B3    | "all?, any?, none? and one?"             | `library/std/iter/tests/aggregate_test.rx` |
| B4, B5, B10, B12 | "take, drop and their combinations" | `library/std/iter/tests/transform_test.rx` |
| B6        | "each and each_with_index visit in order" | `library/std/iter/tests/aggregate_test.rx` |
| B7, B8    | "map, select, filter and find_all"       | `library/std/iter/tests/transform_test.rx` |
| B13, B14  | "chain and zip"                          | `library/std/iter/tests/transform_test.rx` |
| B15, B16  | "zip then to_h builds a Hash"; Set's "to_set from an array and to_a back" | `library/std/iter/tests/transform_test.rx`, `library/std/set/tests/set_test.rx` |
| B17       | `sum_on_string_iter_typeck_rejects`      | `stdlib_iterator.rs`  |
| B18       | _no dedicated pin yet_                   | —                     |
| B19       | "sum, count and reduce, including on an empty array" | `library/std/iter/tests/aggregate_test.rx` |

The `.rx` tests run with `scripts/test-stdlib.sh iter`.

---

## Out of scope (v2)

- Lazy enumeration (`lazy`) that doesn't materialise each step.
- `cycle`, `each_slice`, `each_cons`, `flat_map`, `flatten` — only the
  methods listed above are wired.
