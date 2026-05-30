# Spec — `Drop` semantics & leak prevention

**Source docs:**
[docs/requirements/tier1_04_drop_copy_clone.md](../../requirements/tier1_04_drop_copy_clone.md).

**Status:** shipped Phase 2 #02-#03 (drop elaboration + user `Drop`
include + leak-tracker).

See §3.4a of the canonical surface-syntax spec for the `include Drop`
+ `def var drop` pattern.

Ruxen elaborates drop calls on MIR so that every owned heap value
(`String`, `Array`, `Map`, `Set`, user types that own them) is
released at scope exit.  The `drop_fixtures.rs` test suite uses a
specially-rewritten runtime that counts `free` calls so we can assert
"every alloc has a matching free".

---

## B1 — User `Drop` include runs at scope exit

**Given** a class `T` with `include Drop` and `def var drop ... end`
**When** `let x = T.new(...)` exits its enclosing scope
**Then** `T.drop` is invoked on `x` exactly once, before the
allocation is freed.

## B2 — Reassignment frees the prior value

**Given** `var s = String.from("a")` followed by
`s = String.from("b")`
**Then** the heap buffer for `"a"` is freed at the assignment, not
leaked.

## B3 — Loop-body locals are released across iterations

A binding declared inside a loop body is dropped at the end of each
iteration; the allocation count after N iterations matches the count
after 1 iteration (no per-iteration accumulation).

## B4 — `break` and `continue` drop pending loop-body locals

`break` and `continue` walk back through the active scopes and emit
drop calls for any live owned bindings before transferring control.

## B5 — Owned containers free every element

- `Array[String]` frees every element string when the Array is dropped.
- `Array[Array[Int]]` frees every inner Array.
- `Map[String, Int]` frees every key string.
- `Map[Int, String]` frees every value string.
- `Map[String, Array[Int]]` frees every key and every value.
- `Set[String]` frees every element.

## B6 — `String.push(s)` does not leak the appended slice

`s.push(&other)` clones-and-appends; both the receiver and the
argument are still valid afterwards, and no intermediate buffer
leaks.

## B7 — `String.into_bytes` transfers ownership

The receiver becomes invalid after `into_bytes` (matches B3 in
borrow-check.spec.md); the returned `Array[UInt8]` owns the bytes;
no double-free or leak.

## B8 — `String + String` frees both operands

`a + b` consumes both, produces a new owned `String`, and frees the
backing memory for both `a` and `b`.

## B9 — `runtime_no_leak_fixture` exits cleanly

Baseline: a program that allocates and lets everything drop naturally
must exit with **zero** tracked leaks.  Acts as a regression canary —
any new codegen change that drops a drop call will fail this.

## B10 — Balanced alloc / free counts at exit

For every fixture the suite runs, `allocs == frees` at process exit
(leak tracker reports `0 outstanding`).

---

## Pin tests

| Behaviour | Test fn                                                | File                |
|-----------|--------------------------------------------------------|---------------------|
| B1        | `user_drop_runs_at_scope_exit`                         | `user_drop_runs.rs` |
| B2        | `reassignment_does_not_leak_prior_heap_value`          | `drop_fixtures.rs`  |
| B3        | `loop_body_local_does_not_leak_across_iterations`      | `drop_fixtures.rs`  |
| B4        | `break_drops_loop_body_local` + `continue_drops_loop_body_local` | `drop_fixtures.rs` |
| B5 String | `string_local_is_freed_on_scope_exit`                  | `drop_fixtures.rs`  |
| B5 Array  | `vec_local_is_freed_on_scope_exit` + `vec_of_string_releases_every_element` + `vec_of_vec_int_releases_every_inner_vec` | `drop_fixtures.rs` |
| B5 Map    | `hashmap_local_is_freed_on_scope_exit` + `p04_hashmap_string_to_int_releases_every_key` + `p04_hashmap_int_to_string_releases_every_value` + `p04_hashmap_string_to_vec_int_releases_every_value` | `drop_fixtures.rs` |
| B5 Set    | `p04_hashset_string_releases_every_element`           | `drop_fixtures.rs`  |

<!-- TODO(migration): pin-test fn names still mention `vec_*`, `hashmap_*`, `hashset_*` — internal Rust identifiers, rename when in scope. -->
| B6        | `string_push_does_not_leak`                            | `drop_fixtures.rs`  |
| B7        | `string_into_bytes_transfers_ownership`                | `drop_fixtures.rs`  |
| B8        | `string_plus_op_frees_both_operands`                   | `drop_fixtures.rs`  |
| B9        | `runtime_no_leak_fixture_exits_without_tracked_leaks`  | `drop_fixtures.rs`  |
| B10       | `tracker_reports_balanced_allocs_for_dropped_locals`   | `drop_fixtures.rs`  |

The leak tracker is implemented by a textual substitution at test
compile time (`free(` → `ruxen_test_free(`); see `drop_fixtures.rs`
top-of-file commentary for the sentinel-based safe-list mechanism.

---

## Out of scope (v2)

- `ManuallyDrop[T]` / `MaybeUninit[T]`.
- Asynchronous drops (`AsyncDrop`).
- User-controllable drop ordering across fields (today it's
  declaration order).
- `Drop` for `enum` payloads (today the elaboration assumes each
  variant's payload follows the standard rules; corner cases for
  generic enums are tracked separately).
