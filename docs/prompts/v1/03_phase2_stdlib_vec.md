# 03 — Phase 2 stdlib: full `Vec[T]` surface

**Depends on:** prompt 02 (String surface lands first; some Vec
methods consume String). Drop semantics for Vec via prompt 01A.
**Reads:** `docs/requirements/tier1_01_stdlib.md` §Vec.

## Surface

`Vec[T]`:
- Constructors: `Vec.new`, `Vec.with_capacity(Int)`,
  `Vec.from_iter(I)` where `I: Iterator[Item=T]`.
- Inspectors: `len`, `is_empty`, `capacity`, `get(Int) -> Option[&T]`,
  `first`, `last`, `contains(&T)` (where `T: PartialEq`),
  `iter`, `into_iter`, `iter_mut`.
- Mutators: `push(T)`, `pop -> Option[T]`, `insert(Int, T)`,
  `remove(Int) -> T`, `clear`, `truncate(Int)`, `swap(Int, Int)`,
  `reverse`, `sort` (where `T: Ord`), `sort_by(closure)`, `dedup`
  (where `T: PartialEq`), `extend(I)`, `retain(closure)`.
- Conversions: `clone` (where `T: Clone`), `to_vec`,
  `as_slice -> &[T]`.
- Operators: `==`, `!=`, indexing `v[i]` (panics on OOB).

`&[T]` (slice):
- Inspectors only: `len`, `is_empty`, `get`, `first`, `last`, `iter`,
  `contains`, `starts_with`, `ends_with`.

## TDD

Per method:

1. Unit test in `crates/riven-core/tests/stdlib_vec.rs` with a `.rvn`
   source asserting stdout.
2. E2E fixture `4NN_vec_<op>.rvn` + expected output.
3. Drop-fixtures test for any owning method (push, extend, insert)
   confirms `frees == allocs` for `Vec[String]` (nested heap).

## Implementation

- Backing storage: same layout as today (`riven_vec_*` runtime fns).
- Element type erased to `i64` slot at runtime (same as current); the
  type system enforces correctness statically.
- `Vec[String]`, `Vec[Vec[T]]`, etc. — element-drop must run before
  the outer vec frees its backing array. Add `Vec_drop` runtime fn
  that walks elements and calls their drop, then frees backing.
- `iter` returns a `VecIter[T]` struct with `next -> Option[&T]`.
  Lazy iterator surface is OK here (just iter, not full Iterator
  combinators — those land in prompt 05).
- Closure-taking methods (`sort_by`, `retain`) call into existing
  closure infrastructure (`HirExprKind::Closure`).

## Edge cases

- Push past capacity → realloc. Test that `iter` invalidation rules
  are clear (document: any mutator invalidates outstanding iters;
  enforce via borrow checker — `iter` returns `&mut self` borrow).
- `pop` on empty returns `None`, no panic.
- Indexing out of range panics with `"index N out of range, len M"`.

## Definition of done

- [x] Every method in surface table has positive + negative tests.
  (Batch 1: `with_capacity`, `capacity`, `clear`, `truncate`, `swap`,
  `insert`, `remove`, `extend`, `==`/`!=`, indexing, `pop->Option`,
  `as_slice`, `iter_mut` covered. Batch 2: `from_iter`, `dedup`,
  `sort_by`, `retain`, `Vec[String]` / `Vec[Vec[T]]` drop selector
  wiring; positive fixtures at `tests/release-e2e/cases/40[9-11]_*`,
  negatives in `crates/riven-core/tests/stdlib_vec_negatives.rs`.
  The full `&[T]` slice surface and the lazy `VecIter` cursor class
  remain queued for #05 alongside the trait-driven sort/iterator
  surface.)
- [x] Nested heap (`Vec[String]`, `Vec[Vec[Int]]`) leak tests pass.
  (Batch 1: runtime helpers `riven_vec_drop_string` / `_drop_vec`
  shipped. Batch 2: MIR drop-selector wired in `insert_drops`
  (`crates/riven-core/src/mir/lower.rs`) plus the push-time
  ownership-transfer rule in `compute_dealloc_safe_locals` that
  prevents the source temp from double-freeing the slot. Two
  drop-leak fixtures in `crates/riven-core/tests/drop_fixtures.rs`
  exercise both shapes — `outstanding_allocations == 0` is asserted
  alongside the per-kind free counts.)
- [x] `cargo test --workspace` green (batch 1, batch 2).
- [x] Documented borrow rules for `iter` / mutator interleaving.
  (See `docs/dev/vec_iter_borrow_rules.md` for the receiver-mode
  table and the consume-helper / push-transfer notes for
  implementers.)
- [x] CHANGELOG bullet (batch 1, batch 2).
