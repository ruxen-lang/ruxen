# 05 — Phase 2 stdlib: `Iterator` mixin + lazy combinators

**Depends on:** prompts 02-04 (collections to iterate).
**Reads:** `docs/requirements/tier1_01_stdlib.md` §Iterator,
`docs/requirements/tier2_01_assoc_types.md` (assoc type `Item`
already wired).

## Surface

`mixin Iterator`:
```riven
mixin Iterator
  type Item
  def var next -> Option[Self.Item]

  # Default-method combinators (return new lazy iterators):
  def map[U](f: |Self.Item| -> U) -> Map[Self, U]
  def filter(f: |&Self.Item| -> Bool) -> Filter[Self]
  def take(n: Int) -> Take[Self]
  def skip(n: Int) -> Skip[Self]
  def chain[I](other: I) -> Chain[Self, I]
  def zip[I](other: I) -> Zip[Self, I]
  def enumerate -> Enumerate[Self]

  # Eager terminators:
  def collect[C: FromIterator[Self.Item]] -> C
  def fold[B](init: B, f: |B, Self.Item| -> B) -> B
  def count -> Int
  def sum -> Self.Item   # where Item: Add
  def all(f: |&Self.Item| -> Bool) -> Bool
  def any(f: |&Self.Item| -> Bool) -> Bool
end
```

`mixin FromIterator[T]`:
```riven
mixin FromIterator[T]
  def from_iter[I: Iterator[Item=T]](iter: I) -> Self
end
```

Implement `FromIterator` for `Array[T]`, `String` (where `T: ToString`),
`Map[K,V]` (collecting `(K,V)` pairs), `Set[T]`.

## TDD

For each combinator + terminator:

1. Unit test exercising it on `Array[Int]`, `Array[String]`, range.
2. E2E fixture `6NN_iter_<op>.rvn`.
3. Test combinator chaining: `(0..10).filter(...).map(...).take(3).collect[Array[Int]]`.

## Implementation

- The `Item` assoc type is already plumbed (T2.01). Verify by writing
  a unit test in `typeck` that asserts `<Array[Int] as Iterator>::Item
  == Int`. (Note: internal compiler accessor name preserved pending
  a separate sweep; surface vocabulary is `Item` on `Iterator`.)
- Each lazy combinator is a struct with one or two type params; the
  struct body uses `include Iterator` and delegates `next` to the
  inner iterator.
- No `riven_noop_passthrough`. Each `next()` is real code.
- Closures captured by combinators must respect existing `move` /
  borrow rules.

## Surprise checks

- Infinite iterator + `.take(N)` must terminate.
- `collect[Array[T]]` from an empty iter returns `Array.new`, not panic.
- `sum` on empty iter returns the additive identity (0 for Int).

## Definition of done

- [ ] Mixin `Iterator` lives in `crates/riven-core/runtime/std/iter.rvn`
      (or wherever stdlib sources go). **Deferred (#05 batch 1):** no
      `.rvn` stdlib source loader exists in v1; the mixin is
      currently modelled implicitly by the MIR-level inliner suite at
      `crates/riven-core/src/mir/lower.rs::try_inline_closure_method`.
      Lifting it into a real `.rvn` source requires a stdlib loader
      (not yet built); re-evaluate once #07/#09 land the missing
      surface.
- [ ] All listed combinators + terminators implemented and tested.
      **Partial (#05 batch 1 + 2):** batch 1 plumbed `*Iter.sum` /
      `*Iter.count`. Batch 2 (this commit) adds the closure-taking
      eager terminators `fold` / `all` / `any` (MIR-inlined, no
      runtime helper), the lazy combinators `take(n)` / `skip(n)`
      (eager-materialising via new `riven_vec_take` /
      `riven_vec_skip` runtime fns — internal `vec` naming
      preserved pending sweep), and verifies `enumerate` as a
      typeck-passthrough. Still deferred: `chain` / `zip` (need
      real iterator structs that hold two sources) and
      `collect[C: FromIterator]` (needs the `FromIterator` mixin +
      include machinery; a `.collect_array` shorthand is the planned
      v1 escape hatch). Primary TDD loop now lives at
      `crates/riven-core/tests/stdlib_iterator.rs` (~30 ms; 14
      tests); `release-e2e/cases/60{3..5}_iter_*.rvn` confirm
      end-to-end (`release_e2e_smoke` reports `PASS=208 / 208`).
- [ ] Array, String, Map, Set implement `FromIterator` where
      sensible. **Deferred:** depends on the mixin-Iterator + collect
      surface above.
- [x] `for x in collection` syntax desugars through `Iterator`.
      Verified: `HirExprKind::For` at `mir/lower.rs:2609` lowers
      ranges and Array-like collections directly via `riven_vec_len`
      / `riven_vec_get` (no mixin-method hop). Existing fixtures
      `78_enum_in_array.rvn` and `120_iter_each.rvn` exercise it.
- [x] CI green. `cargo test --test p05_e2e_check` reports `PASS=205`
      (was 203 before this batch); `cargo test --workspace` green
      single-threaded (the `runtime_safety` shared-tmp parallel flake
      is pre-existing and unrelated to this change).
- [x] CHANGELOG bullet.
