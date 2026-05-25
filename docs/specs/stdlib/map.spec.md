# Spec — `Map[K, V]`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.2](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 2 #04-#05; method surface self-hosted in
`library/std/src/map.rx` since #06.8 T#15.

`Map` is a separate-chaining hash table over keys that satisfy
`Hashable + Eq`.  The runtime uses FNV-style hashing; collisions resolve
by walking a linked list per bucket.

---

## B1 — Constructors typecheck

`Map.new()` and the bare literal form `{ k => v, ... }` (per §3.22)
both construct the same type at type-check time.

## B2 — `insert(k, v)` returns the previous value as `Option[V]`

`insert` is the primary write entry point.  Returns `Option.Some(old)`
when the key was present, `nil` otherwise.

(v1 simplification: the type system reports `insert` as `Unit` today;
the runtime returns the old value.  Tracked as a v2 cleanup.)

## B3 — `remove(k)` returns `Option[V]`

Removes the entry if present; returns the removed value.

## B4 — `keys()`, `values()`, `iter()` typecheck as `*Iter[…]`

Each returns an iterator that yields keys, values, or `(K, V)` pairs.

## B5 — `clear()` typechecks as `Unit`

`clear` removes all entries.  Returns nothing (Unit).

## B6 — Equality (`==`) yields `Bool`

`map1 == map2` compares pairwise; order does not matter.

## B7 — `entry(k).or_insert(v)` returns `&var V`

Entry-API insertion-if-absent.  Returns a writable reference to the
entry (existing or freshly inserted).

## B8 — `entry(k).or_insert_with(|| ...)` lazy variant

Same as B7 but only evaluates the closure when the key is absent.

## B9 — Key types: `String` keys + non-hashable rejection

**Given** `Map[String, V]` where `V: Hashable`
**Then** construction typechecks.

**Given** `Map[T, V]` where `T` is not `Hashable` (e.g. nested
Map, plain `Array`)
**Then** typeck emits diagnostic `E0615 type does not implement Hashable`.

**Given** `Map[K, Array[T]]` (Array value, hashable key)
**Then** construction typechecks — only **keys** need `Hashable`.

## B10 — Entry method receiver constraint

`.or_insert(...)` is only callable on the result of `.entry(k)` — not
on a plain `Map` receiver.  Same for `or_insert_with`.

## B11 — Entry method shape: must chain through

`hm.entry(k).or_insert(v)` is the only legal shape.  Splitting into
`let e = hm.entry(k); e.or_insert(v)` is rejected at typeck.

---

## Pin tests

| Behaviour | Test fn                                       | File                          |
|-----------|-----------------------------------------------|-------------------------------|
| B1        | `hashmap_constructors_typecheck`              | `stdlib_map.rs`           |
| B3        | `hashmap_remove_returns_option`               | `stdlib_map.rs`           |
| B4        | `hashmap_keys_values_iter_typecheck`          | `stdlib_map.rs`           |
| B5        | `hashmap_clear_typechecks_as_unit`            | `stdlib_map.rs`           |
| B6        | `hashmap_equality_yields_bool`                | `stdlib_map.rs`           |
| B7        | `hashmap_entry_or_insert_typechecks`          | `stdlib_map.rs`           |
| B8        | `hashmap_entry_or_insert_with_typechecks`     | `stdlib_map.rs`           |
| B9        | `hashmap_with_string_key_is_accepted` + `hashmap_with_vec_value_is_accepted` + `hashmap_with_non_hash_key_emits_e0615` + `hashmap_with_nested_compound_key_emits_e0615` | `stdlib_map_negatives.rs` |
| B10       | `hashmap_or_insert_on_non_entry_receiver_rejected` | `stdlib_map_negatives.rs` |
| B11       | `hashmap_entry_then_or_insert_split_is_rejected` | `stdlib_map_negatives.rs` |

Runtime round-trips covered by E2E fixtures `104_hash_basic.rx` and
`610_iter_collect_map.rx`.

---

## Gaps

- B2: dedicated `hashmap_insert_returns_option` pin is missing
  (typeck reports `Unit` today; v2 cleanup will surface the old
  value through typeck too).

## Out of scope (v2)

- `BTreeMap` / ordered map variant.
- Custom hasher selection (FNV is the v1 default; no SipHash, no
  user-pluggable hashers).
- `Drain`, `IntoIter`, `retain`, `extend` — wired piecemeal as
  needed.
