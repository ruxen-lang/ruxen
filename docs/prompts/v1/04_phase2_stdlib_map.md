# 04 — Phase 2 stdlib: full `Map[K,V]` + `Set[T]`

**Depends on:** prompt 03. Hash mixin derive (prompt 01B) must be
available.
**Reads:** `docs/requirements/tier1_01_stdlib.md` §HashMap
(requirements doc section header pending separate rename — surface
vocabulary is `Map` / `Set`).

## Surface

`Map[K, V]` where `K: Hash + Eq`:
- Constructors: `Map.new`, `Map.with_capacity(Int)`.
- Inspectors: `len`, `is_empty`, `contains_key(&K)`,
  `get(&K) -> Option[&V]`, `keys`, `values`, `iter`.
- Mutators: `insert(K, V) -> Option[V]`, `remove(&K) -> Option[V]`,
  `clear`, `entry(K) -> Entry[K,V]` (or_insert / or_insert_with).
- Operators: `==`, indexing `m[&k]` (panics on missing).

`Set[T]` where `T: Hash + Eq`:
- Constructors: `Set.new`.
- Inspectors: `len`, `is_empty`, `contains(&T)`, `iter`.
- Mutators: `insert(T) -> Bool`, `remove(&T) -> Bool`, `clear`.
- Set-ops: `union`, `intersection`, `difference`.

## TDD

- Unit tests in `crates/riven-core/tests/stdlib_map.rs` and
  `stdlib_set.rs`.
- E2E fixtures `5NN_hashmap_<op>.rvn` and `5NN_hashset_<op>.rvn`
  (fixture-name slugs preserved from the pre-rename era; surface
  vocabulary in fixture *content* is `Map` / `Set`).
- Iteration order tests assert *non*-determinism by running twice
  with different seeds — they must NOT assert a specific order.
- Leak tests: `Map[String, Array[Int]]` populated and dropped,
  `outstanding == 0`.

## Implementation

- Runtime: open-addressing or chaining — implementer's choice. Pick
  the simpler one (chaining via `Array[(K,V)]` per bucket) for v1.
- `Hash` mixin already derivable post prompt 01B. Use the derived
  hasher.
- Capacity grows by 2x when load factor > 0.75. Document.
- `Entry` API uses two enum variants `Occupied(&mut V)` /
  `Vacant(&mut Self, K)` to avoid double lookup.
- `Map_drop`: walk every (K,V), drop each, then free buckets.
- Determinism: hasher is `SipHash13` seeded from a per-process
  random seed. Tests must not rely on order.

## Negative tests

- `Map[Array[Int], V]` — error: `Array` is not `Hash`. Use the
  derive-validation infra.
- Mutating during iteration — borrow-check error.

## Definition of done

- [x] All Map + Set inspector / mutator / constructor methods
      covered by positive release-e2e fixtures
      (`50[1-9]_hashmap_*.rvn`, `52[1-7]_hashset_*.rvn` — legacy
      fixture slugs preserved pending the internal sweep) and
      typecheck-level pin tests
      (`crates/riven-core/tests/stdlib_map.rs`,
      `crates/riven-core/tests/stdlib_set.rs`,
      `crates/riven-core/tests/stdlib_map_negatives.rs`).
- [x] Per-element drop selector for `Map[String, V]` /
      `Set[String]` — landed in batch 2 (commit 45b0e33). Five
      drop helpers (`riven_hash_drop_string_v`,
      `riven_hash_drop_v_string`, `riven_hash_drop_string_string`,
      `riven_hash_drop_v_vec`, `riven_set_drop_string`) walk the
      bucket chains — internal `hash`/`vec` naming preserved
      pending a separate sweep. `mir/lower.rs::insert_drops`
      dispatches on heap-ownership of K/V/T. Four leak regression
      tests in `crates/riven-core/tests/drop_fixtures.rs` pin the
      no-leak property.
- [x] Map indexing operator `m[&k]` (panics on missing key) —
      landed in batch 3. New `riven_hash_index` dispatch in
      `mir/lower.rs` Index handler; surface type changed from
      `Option[V]` to `V` in `typeck/infer.rs::infer_index_ty`.
      Fixture: `tests/release-e2e/cases/509_map_index_op.rvn`.
- [x] Negative tests for non-Hash key constraint — batch 3.
      Resolver now rejects `Map[Array[Int], V]` /
      `Set[Array[Int]]` and every nested-compound variant at the
      type-construction site via `E0615`
      (`resolve/mod.rs::ty_is_valid_hash_key`). Six pin tests in
      `crates/riven-core/tests/stdlib_map_negatives.rs`.
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
      Pin tests in `crates/riven-core/tests/stdlib_map.rs`
      (positive) + `stdlib_map_negatives.rs` (rejection); e2e
      fixtures `510_map_entry_or_insert.rvn` and
      `511_map_entry_or_insert_with.rvn`.
- [x] `cargo test --workspace` green — 0 failed across all 30+ test
      binaries including the new fixtures. The release-e2e suite
      runs every `tests/release-e2e/cases/*.rvn` end-to-end and
      diffs stdout against `expected/*.out`.
- [x] CHANGELOG bullet under `## [Unreleased] ### Added`.
