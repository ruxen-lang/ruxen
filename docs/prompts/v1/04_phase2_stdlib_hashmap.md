# 04 — Phase 2 stdlib: full `HashMap[K,V]` + `HashSet[T]`

**Depends on:** prompt 03. Hash trait derive (prompt 01B) must be
available.
**Reads:** `docs/requirements/tier1_01_stdlib.md` §HashMap.

## Surface

`HashMap[K, V]` where `K: Hash + Eq`:
- Constructors: `HashMap.new`, `HashMap.with_capacity(Int)`.
- Inspectors: `len`, `is_empty`, `contains_key(&K)`,
  `get(&K) -> Option[&V]`, `keys`, `values`, `iter`.
- Mutators: `insert(K, V) -> Option[V]`, `remove(&K) -> Option[V]`,
  `clear`, `entry(K) -> Entry[K,V]` (or_insert / or_insert_with).
- Operators: `==`, indexing `m[&k]` (panics on missing).

`HashSet[T]` where `T: Hash + Eq`:
- Constructors: `HashSet.new`.
- Inspectors: `len`, `is_empty`, `contains(&T)`, `iter`.
- Mutators: `insert(T) -> Bool`, `remove(&T) -> Bool`, `clear`.
- Set-ops: `union`, `intersection`, `difference`.

## TDD

- Unit tests in `crates/riven-core/tests/stdlib_hashmap.rs` and
  `stdlib_hashset.rs`.
- E2E fixtures `5NN_hashmap_<op>.rvn` and `5NN_hashset_<op>.rvn`.
- Iteration order tests assert *non*-determinism by running twice
  with different seeds — they must NOT assert a specific order.
- Leak tests: `HashMap[String, Vec[Int]]` populated and dropped,
  `outstanding == 0`.

## Implementation

- Runtime: open-addressing or chaining — implementer's choice. Pick
  the simpler one (chaining via `Vec[(K,V)]` per bucket) for v1.
- `Hash` trait already derivable post prompt 01B. Use the derived
  hasher.
- Capacity grows by 2x when load factor > 0.75. Document.
- `Entry` API uses two enum variants `Occupied(&mut V)` /
  `Vacant(&mut Self, K)` to avoid double lookup.
- `HashMap_drop`: walk every (K,V), drop each, then free buckets.
- Determinism: hasher is `SipHash13` seeded from a per-process
  random seed. Tests must not rely on order.

## Negative tests

- `HashMap[Vec[Int], V]` — error: `Vec` is not `Hash`. Use the
  derive-validation infra.
- Mutating during iteration — borrow-check error.

## Definition of done

- [x] All HashMap + HashSet inspector / mutator / constructor methods
      covered by positive release-e2e fixtures (`50[1-9]_hashmap_*.rvn`,
      `52[1-7]_hashset_*.rvn`) and typecheck-level pin tests
      (`crates/riven-core/tests/stdlib_hashmap.rs`,
      `crates/riven-core/tests/stdlib_hashset.rs`,
      `crates/riven-core/tests/stdlib_hashmap_negatives.rs`).
- [x] Per-element drop selector for `HashMap[String, V]` /
      `HashSet[String]` — landed in batch 2 (commit 45b0e33). Five
      drop helpers (`riven_hash_drop_string_v`,
      `riven_hash_drop_v_string`, `riven_hash_drop_string_string`,
      `riven_hash_drop_v_vec`, `riven_set_drop_string`) walk the
      bucket chains; `mir/lower.rs::insert_drops` dispatches on
      heap-ownership of K/V/T. Four leak regression tests in
      `crates/riven-core/tests/drop_fixtures.rs` pin the no-leak
      property.
- [x] HashMap indexing operator `m[&k]` (panics on missing key) —
      landed in batch 3. New `riven_hash_index` dispatch in
      `mir/lower.rs` Index handler; surface type changed from
      `Option[V]` to `V` in `typeck/infer.rs::infer_index_ty`.
      Fixture: `tests/release-e2e/cases/509_hashmap_index_op.rvn`.
- [x] Negative tests for non-Hash key constraint — batch 3. Resolver
      now rejects `HashMap[Vec[Int], V]` / `HashSet[Vec[Int]]` and
      every nested-compound variant at the type-construction site
      via `E0615` (`resolve/mod.rs::ty_is_valid_hash_key`). Six pin
      tests in `crates/riven-core/tests/stdlib_hashmap_negatives.rs`.
- [x] `Entry[K,V]` API (`entry(K).or_insert(V) /
      .or_insert_with { || V }`) — landed via single-MIR-unit chain
      detection. Both typeck (`infer.rs` MethodCall handler) and
      MIR (`lower.rs::inline_entry_or_insert`) recognise the chain
      atomically; the inlined emission is `if !riven_hash_contains_key(m, k)
      { riven_hash_insert(m, k, v); }`, honouring the lazy-default
      contract of `or_insert_with`. Chain returns `Unit` (v1
      simplification — Rust's `&mut V` return required pointer-
      dispatch infrastructure deferred to v2). Splitting the chain
      across statements is rejected by typeck with a clear error.
      Pin tests in `crates/riven-core/tests/stdlib_hashmap.rs`
      (positive) + `stdlib_hashmap_negatives.rs` (rejection); e2e
      fixtures `510_hashmap_entry_or_insert.rvn` and
      `511_hashmap_entry_or_insert_with.rvn`.
- [x] `cargo test --workspace` green — 0 failed across all 30+ test
      binaries including the new fixtures. The release-e2e suite
      runs every `tests/release-e2e/cases/*.rvn` end-to-end and
      diffs stdout against `expected/*.out`.
- [x] CHANGELOG bullet under `## [Unreleased] ### Added`.
