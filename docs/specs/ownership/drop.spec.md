# Spec — `Drop` semantics & leak prevention

**Source docs:**
[docs/requirements/tier1_04_drop_copy_clone.md](../../requirements/tier1_04_drop_copy_clone.md).

**Status:** shipped Phase 2 #02-#03 (drop elaboration + user `Drop`
include + leak-tracker).

See §3.4a of the canonical surface-syntax spec for the `include Drop`
+ `def var drop` pattern.

Ruxen elaborates drop calls on MIR so that every owned heap value
(`String`, `Array`, `Hash`, `Set`, user types that own them) is
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

**Given** `var s = "a"` followed by
`s = "b"`
**Then** the heap buffer for `"a"` is freed at the assignment, not
leaked.

## B3 — Loop-body locals are released across iterations

A binding declared inside a loop body is dropped at the end of each
iteration; the allocation count after N iterations matches the count
after 1 iteration (no per-iteration accumulation).

For a built-in heap local (`String`, `Array`, `Hash`, `Set`, including a
compiler temporary such as an interpolation result) the value it held is
freed when it is redefined, provided it owned every value it was given
and none was moved out; a value moved into a collection is never freed.

## B4 — `break` and `continue` drop pending loop-body locals

`break` and `continue` walk back through the active scopes and emit
drop calls for any live owned bindings before transferring control.

## B5 — Owned containers free every element

- `Array[String]` frees every element string when the Array is dropped.
- `Array[Array[Int]]` frees every inner Array.
- `Hash[String, Int]` frees every key string.
- `Hash[Int, String]` frees every value string.
- `Hash[String, Array[Int]]` frees every key and every value.
- `Set[String]` frees every element.
- `Array[C]` and `Hash[K, C]` (C a non-generic class or struct with a
  `clone`) run each element's `def drop` and free it. The slot's
  descriptor is `RUXEN_ELEM_OBJ` plus the address of per-type element glue.
  A `Set[C]` element and a `Hash[C, V]` key stay unowned (their table copies
  keys by key kind), as do the elements of a generic class or an enum.
- A container whose element pointer escaped (an element read stored into
  another collection, a field, a consuming parameter, or the return value)
  frees only its spine: the element has another owner or none, never two.

A class instance does not free its own fields (there is no per-field drop
yet; see `docs/decisions/drop-elaboration.md`), so a `String` field of an
element leaks with or without B5.

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

## B11 — Owned closure captures are released

A closure pair owns its captures block. At closure drop, direct by-value heap
captures that are proven still owned are dropped in reverse capture order, with
each pointer NULL-guarded. The captures block is then freed, followed by the
closure pair. The synthetic `__block`/`&block` parameter borrows the pair for
the yielding call, so the caller performs this cleanup.

Borrowed captures are never freed by closure drop. A capture returned from the
closure, stored into another aggregate, explicitly moved, or passed to a
consuming/ambiguous call is treated as escaped and is not field-dropped.

---

## Pin tests

| Behaviour | Test fn                                                | File                |
|-----------|--------------------------------------------------------|---------------------|
| B1        | e2e `134_user_drop_runs.rx`                            | release-e2e         |
| B2        | `reassignment_does_not_leak_prior_heap_value`          | `drop_fixtures.rs`  |
| B3        | `loop_body_local_does_not_leak_across_iterations`      | `drop_fixtures.rs`  |
| B4        | `break_drops_loop_body_local` + `continue_drops_loop_body_local` | `drop_fixtures.rs` |
| B5 String | `string_local_is_freed_on_scope_exit`                  | `drop_fixtures.rs`  |
| B5 Array  | `vec_local_is_freed_on_scope_exit` + `vec_of_string_releases_every_element` + `vec_of_vec_int_releases_every_inner_vec` | `drop_fixtures.rs` |
| B5 Hash   | `hashmap_local_is_freed_on_scope_exit` + `p04_hashmap_string_to_int_releases_every_key` + `p04_hashmap_int_to_string_releases_every_value` + `p04_hashmap_string_to_vec_int_releases_every_value` | `drop_fixtures.rs` |
| B5 Set    | `p04_hashset_string_releases_every_element`           | `drop_fixtures.rs`  |

<!-- TODO(migration): pin-test fn names still mention `vec_*`, `hashmap_*`, `hashset_*` — internal Rust identifiers, rename when in scope. -->
| B5 class  | `class_elements_and_hash_class_values_are_freed_with_the_container` + `an_element_alias_stored_elsewhere_is_not_freed_twice`; e2e `2126`, `2127` | `drop_fixtures.rs` |
| B3 rebind | `borrow_in_loop_frees_every_iteration_no_uaf` + `loop_local_moved_into_hash_is_not_freed_on_rebind` + `string_locals_in_monomorphised_bodies_are_freed`; e2e `2129` | `drop_fixtures.rs` |
| Interpolation | `interpolation_temporaries_are_freed_in_a_loop`; e2e `2128` | `drop_fixtures.rs` |
| B6        | `string_push_does_not_leak`                            | `drop_fixtures.rs`  |
| B7        | `string_into_bytes_transfers_ownership`                | `drop_fixtures.rs`  |
| B8        | `string_plus_op_frees_both_operands`                   | `drop_fixtures.rs`  |
| B9        | `runtime_no_leak_fixture_exits_without_tracked_leaks`  | `drop_fixtures.rs`  |
| B10       | `tracker_reports_balanced_allocs_for_dropped_locals`   | `drop_fixtures.rs`  |
| B11       | `closure_and_block_captures_are_freed_at_closure_drop` + `closure_drop_does_not_free_a_moved_out_capture` | `drop_fixtures.rs` |

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
- Precise drop flags for conditionally escaped closure captures. Nested
  by-value re-captures and ambiguous consuming calls currently suppress the
  capture-field drop, preferring a bounded leak to a possible double-free.
