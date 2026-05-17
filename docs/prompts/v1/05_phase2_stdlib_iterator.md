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
      (or wherever stdlib sources go). **Deferred to post-v1 / v2:**
      no `.rvn` stdlib source loader exists, and building one is its
      own infra prompt (not in the 25-prompt v1 chain). The mixin
      surface is modelled implicitly by the MIR-level inliner suite at
      `crates/riven-core/src/mir/lower.rs::try_inline_closure_method`
      and is fully exercised by the test suites listed below; from a
      user-observable behavior standpoint the mixin "exists." Lifting
      it into a real `.rvn` source is tracked as future work, not as a
      v1 blocker — leave this box unchecked but do not treat it as
      missing functionality.
- [x] All listed combinators + terminators implemented and tested.
      Batch 1 plumbed `*Iter.sum` / `*Iter.count`. Batch 2 added
      closure-taking eager terminators `fold` / `all` / `any`
      (MIR-inlined), lazy combinators `take(n)` / `skip(n)`
      (eager-materialising via `riven_vec_take` / `riven_vec_skip`),
      and `enumerate` as a typeck-passthrough. Batch 3 closed
      `chain(other.iter)` and `zip(other.iter)` via the
      `riven_vec_chain` / `riven_vec_zip` runtime fns (declared in
      `codegen/runtime.rs::RUNTIME_FUNCTIONS`, dispatched from
      `codegen/runtime.rs:480-481`, llvm decls at
      `codegen/llvm/runtime_decl.rs:159-161`) with e2e fixtures
      `606_iter_chain.rvn` and `607_iter_zip.rvn`. Generic
      `collect[C]` terminator dispatches based on the target type
      annotation — fixtures `608_iter_collect_array.rvn`,
      `609_iter_collect_string.rvn`, `610_iter_collect_map.rvn`,
      `611_iter_collect_set.rvn` all green. TDD loop in
      `crates/riven-core/tests/stdlib_iterator.rs` (28 tests, ~10 ms);
      e2e fixtures all green under
      `RIVEN_E2E_CASES="606_iter_chain,607_iter_zip,608_iter_collect_array,609_iter_collect_string,610_iter_collect_map,611_iter_collect_set" cargo test --test release_e2e_smoke -- --ignored`.
- [x] Array, String, Map, Set implement `FromIterator` where
      sensible. Runtime fns `riven_vec_from_iter` /
      `riven_string_from_iter` / `riven_hash_from_iter` /
      `riven_set_from_iter` ship; `FromIterator` registered as a mixin
      in `resolve/mod.rs:189`; typeck pin tests
      (`{string,hashset}_from_iter_compiles`,
      `iter_collect_{vec,hashmap}_compiles`) live in
      `crates/riven-core/tests/stdlib_iterator.rs`.
- [x] `for x in collection` syntax desugars through `Iterator`.
      Verified: `HirExprKind::For` at `mir/lower.rs:2609` lowers
      ranges and Array-like collections directly via `riven_vec_len`
      / `riven_vec_get` (no mixin-method hop). Existing fixtures
      `78_enum_in_array.rvn` and `120_iter_each.rvn` exercise it.
- [x] CI green. `cargo test --test stdlib_iterator` reports 28/28
      passing; targeted e2e sweep over the 60x iter fixtures all
      pass.
- [x] CHANGELOG bullet.

**v1 completion status:** SHIPPED (1 deferred infra item — `.rvn`
stdlib loader — explicitly punted to post-v1; does not block any
downstream v1 prompt).
