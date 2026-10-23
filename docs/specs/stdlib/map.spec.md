# Spec — `Hash[K, V]`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.2](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 2 #04-#05; method surface self-hosted in
`library/std/map/src/lib.rx` since #06.8 T#15.

`Hash` is a separate-chaining hash table over keys that satisfy
`Hashable + Eq`.  The runtime uses FNV-style hashing; collisions resolve
by walking a linked list per bucket.

---

## B1 — Constructors typecheck

`Hash.new()` and the bare literal form `{ k => v, ... }` (per §3.22)
both construct the same type at type-check time.

## B2 — `insert(k, v)` returns the previous value as `Option[V]`

`insert` is the primary write entry point.  Returns `Option.Some(old)`
when the key was present, `nil` otherwise.

(v1 simplification: the type system reports `insert` as returning `nil` today;
the runtime returns the old value.  Tracked as a v2 cleanup.)

## B3 — `remove(k)` returns `Option[V]`

Removes the entry if present; returns the removed value.

## B4 — `keys`, `values`, `to_a` return arrays

`keys` returns `Array[&K]`, `values` returns `Array[&V]`, and `to_a`
returns the `(K, V)` pairs as `Array[(K, V)]`.

## B5 — `clear` returns `nil`

`clear` removes all entries and returns `nil`.

## B6 — Equality (`==`) yields `Bool`

`map1 == map2` compares pairwise; order does not matter.

## B7 — `entry(k).or_insert(v)` returns `&var V`

Entry-API insertion-if-absent.  Returns a writable reference to the
entry (existing or freshly inserted).

## B8 — `entry(k).or_insert_with(|| ...)` lazy variant

Same as B7 but only evaluates the closure when the key is absent.

## B9 — Key types: `String` keys + non-hashable rejection

**Given** `Hash[String, V]` where `V: Hashable`
**Then** construction typechecks.

**Given** `Hash[T, V]` where `T` is not `Hashable` (e.g. nested
Hash, plain `Array`)
**Then** typeck emits diagnostic `E0615 type does not implement Hashable`.

**Given** `Hash[K, Array[T]]` (Array value, hashable key)
**Then** construction typechecks — only **keys** need `Hashable`.

## B10 — Entry method receiver constraint

`.or_insert(...)` is only callable on the result of `.entry(k)` — not
on a plain `Hash` receiver.  Same for `or_insert_with`.

## B11 — Entry method shape: must chain through

`hm.entry(k).or_insert(v)` is the only legal shape.  Splitting into
`let e = hm.entry(k); e.or_insert(v)` is rejected at typeck.

---

## Pin tests

| Behaviour | Test fn                                       | File                          |
|-----------|-----------------------------------------------|-------------------------------|
| B1        | "with_capacity, clear and empty?"             | `library/std/map/tests/mutate_test.rx` |
| B3        | "remove returns the removed value"            | `library/std/map/tests/mutate_test.rx` |
| B4        | "keys and values"                             | `library/std/map/tests/lookup_test.rx` |
| B5        | "with_capacity, clear and empty?"             | `library/std/map/tests/mutate_test.rx` |
| B6        | "== compares entries regardless of insertion order" | `library/std/map/tests/lookup_test.rx` |
| B7        | "entry.or_insert keeps the first value"       | `library/std/map/tests/mutate_test.rx` |
| B8        | "entry.or_insert_with runs the block only for a new key" | `library/std/map/tests/mutate_test.rx` |
| B9        | `hashmap_with_string_key_is_accepted` + `hashmap_with_vec_value_is_accepted` + `hashmap_with_non_hash_key_emits_e0615` + `hashmap_with_nested_compound_key_emits_e0615` | `stdlib_map_negatives.rs` |
| B10       | `hashmap_or_insert_on_non_entry_receiver_rejected` | `stdlib_map_negatives.rs` |
| B11       | `hashmap_entry_then_or_insert_split_is_rejected` | `stdlib_map_negatives.rs` |

The `.rx` tests run with `scripts/test-stdlib.sh map`.

---

## Gaps

- B2: dedicated `hashmap_insert_returns_option` pin is missing
  (typeck reports `nil` today; v2 cleanup will surface the old
  value through typeck too).

## Out of scope (v2)

- A sorted-key map variant.
- Custom hasher selection (FNV is the v1 default; no SipHash, no
  user-pluggable hashers).
- `delete_if`, `keep_if`, `merge` — wired piecemeal as needed.
