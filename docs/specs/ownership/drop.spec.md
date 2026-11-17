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

A closure pair owns its captures block. What it owns inside that block
depends on its kind (`docs/decisions/blocks-procs-lambdas.md`):

- A **block** (a trailing block bound to a `Fn[...]` parameter) borrows every
  capture. Its drop frees only the captures block and the pair; the enclosing
  scope frees each captured local once. A `Fn[...]` parameter borrows the pair
  for the call, so the caller frees it afterwards. A String or scalar capture
  is held by the address of the enclosing variable's own storage (no heap
  cell), so an assignment, `op=`, `&var` argument or `def var` call in the body
  replaces the variable itself, freeing the value it replaces once.
- A **proc** or **lambda** owns its by-value captures. At closure drop, captures
  that are proven still owned are dropped in reverse capture order, each
  pointer NULL-guarded, then the captures block, then the pair. The borrow
  checker records the capture as a move, so no other owner can free it. A
  String capture lives in the closure's own slot: a write, `&var` or `def var`
  call in the body replaces the value in that slot (`FieldAddr`), and the
  closure's drop frees whatever the slot holds last.
- Inside any closure body the captures are borrowed (E1019 on a move), so a
  proc or lambda called twice never hands the same capture out twice, and a
  closure nested in an owning lambda owns a clone made in the outer body
  rather than re-capturing the lambda's slot.

Borrowed captures are never freed by closure drop. A capture returned from the
closure, stored into another aggregate, explicitly moved, or passed to a
consuming/ambiguous call is treated as escaped and is not field-dropped.

## B12 — A field or collection frees the closure it holds

A closure value is a pointer to a four-word pair (`mir/lower/drops/`,
closure pairs): `fn_ptr`, `captures_ptr`, `drop_fn` (the literal's own
`<closure>_drop`) and `shares` (owners beyond the first). Because the pair
carries its drop function, an owner that never saw the literal can free it:

- Storing a lambda or proc into a field, an `Array` or a `Hash` value
  moves it in; the creating scope no longer drops it.
- A closure-typed field (`Fn[...]`, `Proc[...]`, `some Fn[...]`) is owned: the
  object's field glue, and a store that replaces it, release it through
  `__ruxen_closure_drop(pair)`, which calls `pair[drop_fn](pair)`.
- A collection slot of closure type is described by the `RUXEN_ELEM_CLOSURE`
  element tag; `ruxen_elem_drop` calls the pair's drop slot.
- `<closure>_drop` releases one owner: while `shares > 0` it decrements and
  returns; the last owner frees the owned captures, the captures box and the
  pair. A Block pair frees only the box and the pair (its captures are
  borrowed).
- Copies share, never duplicate: `.clone` of a `Proc` (`Closure#clone`, the C `ruxen_closure_share`),
  a derived `clone` of a closure field, and `ruxen_elem_clone` of a CLOSURE slot
  (`Array#drop`, a derived clone of an `Array[Proc]` field) each add one owner.
  Reading a closure field is a borrow (`&Proc[...]`): it may be called or lent to
  a `Fn[...]` parameter, and moving it into an owned slot is E0744.

## B13 — A call through a closure lends its arguments

The element-escape pass (`demote_escaping_element_drops`) makes a container's
drop shallow when an element alias may be kept by another owner. A call
through a closure pair keeps an argument only if a closure body that can be
the target stores that parameter (`select`'s inner block pushes its argument
into the result; `puts(prefix + s)` does not). The pass tracks which closure
literals each local, parameter and capture slot may hold (`closure_targets`)
and computes summaries per (function, closure context), so `Bag#each` calling
a block with each of `@items` keeps `@items` deep while `@items.select { }`
still demotes. Argument 0, the pair's captures box, is owned by the pair and
never counts as an escape. A pair of unknown origin (read out of a field, a
collection or a call result, or a parameter of an exported, vtable or
address-taken function) may hold any closure, so its calls keep demoting.

## B14 — A matched payload has one owner

A `match` / `if let` / `case ... in` arm that binds a heap payload of an
`Option` or `Result` by value moves it into the binding, which frees it at
scope exit. The borrow checker records the move of a scrutinee variable, so
the variable is not matched or read again (E1001). When the scrutinee is a
`let` / `var` local the frame owns (not a parameter, not a field read or
reference), the arm also frees the emptied box (`MirPayloadMove`); an arm
with a guard keeps the box, since a failed guard falls through to another arm.
A field read or a reference only views its payload: the binding does not free
it. A `_` payload frees the payload only when the scrutinee is a temporary,
never when it names a place.

## B15 — Statement temporaries are dropped at the end of their statement

**Given** a temporary that owns a fresh value and is not moved (returned,
stored, passed by value): a built-in heap value, a class or struct instance, or
an instance of a class with a `def drop` (including FFI drop classes such as
`MutexGuard`)
**Then** it is dropped at the end of the statement that created it:
`m.lock!.get` releases the lock before the next statement, and a temporary in a
loop body is freed every iteration. A value returned by a function is owned by
the caller when every return of that function is a fresh allocation its own
frame owned (a container only when its elements own no heap); a getter that
returns a field stays borrowed.

## B16 — A borrow of a temporary keeps it alive for the borrowing binding

**Given** a binding that borrows storage a temporary owns: a field read
(`n = U.new("t").name`, `mk(4).name`), a method or function returning a
reference into it (`U.new("t").label`, `pick(U.new("t"))` with
`pick(u: &U) -> &String`), or an element read
**Then** the temporary is not dropped at the end of its statement: it lives
while the binding (and any copy of it) is used, and is dropped once after the
binding's last use: at the end of the statement that last uses it when that
is in the same block (in a loop body, every iteration), else at function exit.
Writing or moving a borrowed *variable* root while the borrow is live is a
compile-time E1009 (`x = u.name; u.name = "n"; puts x`), not a run-time
concern (TODO.md Decisions, "Borrow of a temporary"). The borrow follows
every shape that yields a place (an `if`/`case` arm, a block tail, a cast, a
parallel assignment's value, a copy of the borrow, a closure capturing it, a
pattern binding, a `for` loop over a field), and every write, `var self`
call, `&var` argument and move checks the field path it changes (the MIR
loan check, `mir/loan_check.rs`), so the run time never sees a write under a
live borrow.

## B17 — A `&var` of a place points at the place's own storage

**Given** a place passed to a slot-addressed `&var T` parameter or bound by
`r = &var place`: a variable, a field (`replace(&var u.name)`,
`replace(u.name)`), an Array element (`replace(&var a[0])`), or a variable a
closure captured
**Then** the address is the place's own storage, never a copy stored back:
a local's slot, a cell, a field's slot in its object (`MirInst::FieldAddr`),
an element's slot (`ruxen_vec_slot_addr`), the enclosing variable a block
borrows, or the slot a proc or lambda owns. The object or Array is evaluated
once. The callee (or a write through `r`) frees the value it replaces through
the address, once. The same address serves a `&var` field receiver
(`(&var u.name).push_str(s)`), an owned String field or element grown in
place (`u.name += s`, `b[0] += s`, `u.name.push_str(s)`), and `op=` on an
Array element (`c[0] += 5`); `op=` on a place with no address is a lowering
error, never a silent no-op. A place with no address (a Hash value, a
safe-navigation field) passed `&var` is rejected at compile time (E1021).
While the call runs, the place's root is mutably borrowed: no other operand
of the call may reach it (E1002), and a `r = &var place` binding keeps the
place borrowed until the last write through `r` (E1009 on replacing its
owner before then).

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
| B11       | `closure_and_block_captures_are_freed_at_closure_drop` + `closure_drop_does_not_free_a_moved_out_capture` + `cloned_capture_moves_pass_borrow_check_and_free_once` | `drop_fixtures.rs` |
| B12       | `an_object_frees_the_lambda_in_its_field` + `an_array_frees_the_lambdas_it_holds` + `a_hash_frees_the_procs_it_holds` + `a_shared_proc_is_freed_once_by_its_last_owner`; `field_ownership::a_closure_field_read_is_a_borrow_shared_by_clone`; e2e `2201` | `drop_fixtures.rs` |
| B13       | `a_block_called_from_a_for_loop_borrows_the_elements` + `a_field_element_kept_by_its_replacement_is_not_freed_twice`; e2e `2201` | `drop_fixtures.rs` |
| B14       | `if_let_binding_owns_the_option_payload` + `match_on_a_place_keeps_its_payload` | `drop_fixtures.rs` |
| B15       | `heap_statement_temporaries_are_freed` + `drop_class_statement_temporary_drops_at_statement_end`; e2e `2210`, `2215` | `drop_fixtures.rs` |
| B16       | `borrow_of_a_temporary_keeps_it_alive_then_frees_it` + `place_writes_after_borrows_neither_leak_nor_double_free`; `borrowed_places::*` (E1009, E1002); e2e `2219`; the loan matrix `tmp/test-cache/loan-matrix/gen.py` and `gen2.py` | `drop_fixtures.rs` |
| B17       | `var_string_param_writes_neither_leak_nor_double_free` + `var_field_receiver_grows_the_field_in_place` + `var_places_point_at_the_real_storage` + `compound_assignment_updates_an_array_element` + `compound_assignment_to_a_hash_value_is_rejected`; `borrowed_places::semantics_a_hash_value_cannot_be_a_var_argument` (E1021), `borrowed_places::semantics_a_var_binding_keeps_its_place_borrowed_through_writes` (E1009); e2e `2219`, `2221` | `drop_fixtures.rs` |

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
- `Hash#each` / `each_key` / `each_value` leak: `ruxen_hash_entries` allocates
  a 16-byte tuple per entry that the entries Array (POD element descriptor)
  never frees, and the element-escape pass treats that Array's drop as freeing
  aliases of the Hash's keys and values, so the iterated Hash's own drop
  becomes shallow. Open work (found while pinning B12).
- A by-value `Proc` argument the callee never stores is not released by the
  callee (F-25), so a shared pair passed that way keeps one owner too many.
- Precise drop flags for conditionally escaped closure captures. Nested
  by-value re-captures and ambiguous consuming calls currently suppress the
  capture-field drop. This is a leak and is open work: the owner's rule
  (2026-10-17) is that Ruxen code never leaks, so a leak is never an accepted
  trade for avoiding a double free.
