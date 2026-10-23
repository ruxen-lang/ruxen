# ADR: Blocks, procs and lambdas own or borrow their captures

Status: Accepted (2026-10-17, owner decision)
Branch: `feat/concurrency-adoption`
Settles: spec OQ-5 (`Proc[...]` is the stored-closure type).
Builds on: `ruby-block-semantics.md` (`&block`, `yield`, `block_defined?`).

## Context

Every closure owned its by-value captures, and the borrow checker treated a
plain closure's captures as shared borrows. The two disagreed whenever two
closures captured the same non-Copy local: both drop glues freed it.

```ruby
prefix = "p-"
bag.each { |n| puts(prefix + n) }   # user each(&block)
bag.each { |n| puts(prefix + n) }   # second free of prefix
```

glibc survived the second free; the macOS allocator did not (`sync` failed in CI
on macos-14). Guard Malloc (`DYLD_INSERT_LIBRARIES=/usr/lib/libgmalloc.dylib`)
reproduces it on any Mac.

The owner's constraint: Ruxen code must never leak, without a GC. A fix that
trades the double free for a leak is rejected.

## Decision

Ruby's three kinds, each with one ownership rule.

| Kind | How it is written | Captures | Lifetime |
|---|---|---|---|
| Block | a trailing `{ }` / `do ... end` bound to a `Fn[...]` parameter (including `&block` and implicit `yield`) | borrowed; the enclosing scope frees them once | the call only; the caller frees the closure pair after the call |
| Proc | `proc { }`, `Proc.new { }`, or a trailing block bound to a `Proc[...]` / `any Fn[...]` parameter | owned; capturing a non-Copy variable moves it, so reuse is E1001 | storable; freed by whatever owns it |
| Lambda | `->(x) { }`, `->(x) do ... end`, `lambda { }`, and any other closure literal used as a value | owned, as for Proc | storable |

- `move` keeps meaning "owns every capture".
- `Proc[(A) -> R]` is the same type as `any Fn[Fn(A) -> R]`.
- Typeck settles a trailing block's kind from the parameter it binds to. An
  unknown target (no signature, a type parameter) becomes a Proc, which owns
  and cannot dangle.
- A `Fn[...]` parameter may only be called, tested against `nil`
  (`block_given?`), or forwarded to another `Fn[...]` parameter. Any other use
  stores the block and is **E1018**; declare the parameter `Proc[...]` instead.
- `ruxen fmt` prints a bare closure value as `->(params) { ... }`.
- One rule for every kind: a closure body may read its captures but not move
  them. The enclosing scope frees a block's captures; a proc or lambda frees
  its own on drop, so a body that moved one would hand the same value out on
  every call (a double free on the second call). Returning one, binding it,
  assigning it, casting it (`s as String`), binding it in a `match` / `if let`
  pattern, a multi-assign value, a literal element or field, passing it to a
  by-value parameter or `consume self` receiver, or capturing it in an owning
  proc or lambda is **E1019**, and the fix is `.clone`. The borrow checker
  enforces it where it records every move (`record_move`), so no position
  needs its own check. A proc or lambda nested in an owning lambda therefore
  owns a clone made in the outer body (`let mine = s.clone`), never the outer
  lambda's slot; a block nested anywhere borrows.
- A by-value pattern binding of a non-Copy payload moves the scrutinee
  variable (`if let Some(v) = o` twice is E1001). A Copy payload, `_`, a
  literal pattern or a temporary scrutinee moves nothing.
- A block or proc literal may take fewer parameters than its `Fn` type (Ruby
  arity); the extra arguments are dropped. A lambda keeps exact arity (E0743).
- A proc or lambda passed to a `Send`-bounded parameter owns its captures, so
  it borrows no stack local; a block bound there is E1013.

Lowering: a Block's capture slots hold a `MirInst::Ref` of the outer local, so
the store does not taint the source and the closure drop frees no capture. A
Proc or Lambda stores the local and frees its owned captures in its drop glue
(`synthesize_closure_drop`).

## Alternatives rejected

- **No owner for a shared capture.** Sound but leaks; rejected by the no-leak rule.
- **Copy the capture into each closure.** Leak-free for Clone types, but a
  block that mutates a String would change a private copy (`buf.push_str` in one
  block invisible to another), and non-Clone types have no copy.
- **Plain closures borrow, with no escape check.** A stored closure (quiver's
  `button` handlers) would point at a freed local.
- **A second capture is always E1001.** Rejects everyday Ruby: one variable
  used in two `each` blocks.
- **Copy a capture implicitly when a block moves it.** Hides an allocation
  and only works for Clone types; an explicit `.clone` (E1019's hint) keeps
  the cost visible.

## Consequences

- Two blocks may share any local; nothing leaks and nothing is freed twice.
- Procs and lambdas that share a value need `.clone` (for `SharedSync` and
  quiver's `State` this is a refcount bump). So does a proc created inside a
  block or an owning lambda from one of its captures (quiver's `button`
  handlers inside a layout block or a `list` closure clone their `State`), and
  a proc or lambda body that hands its own capture to a by-value parameter.
- Stdlib call-only parameters moved from `any Fn[...]` to `Fn[...]`; stored
  ones use `Proc[...]`.

## Closures in fields and collections (cards f4, f5, 2026-10-17)

- A lambda or proc stored in a field or an `Array` / `Hash` element moves in
  and is freed once by that owner. The closure pair carries its literal's drop
  function (word 2) and an owner count (word 3), so field glue and the
  `RUXEN_ELEM_CLOSURE` element tag can free a closure whose literal they never
  saw (`docs/specs/ownership/drop.spec.md` B12).
- Reading a closure field is a borrow, like any non-Copy field. `.clone` of a
  `Proc` shares the pair (one more owner; the last one frees it), and so do a
  derived `clone` of a closure field and a copied `Array[Proc]`. A `Fn[...]`
  value has no `.clone`: it may be a block that borrows its captures.
- A call through a closure lends its arguments unless a closure body that can
  be the target stores them (B13), so a user `each(&block)` over its own Array
  no longer leaks that Array's elements.

## Open (tracked in TODO.md)

- Card "Ruby lambdas" (after batch 3): counted shared captures so procs and
  lambdas need no `.clone`, with a cycle rule so they still cannot leak; a
  proc's `return` leaving the enclosing method; `.call`, `[]` and `lambda?`.
  (Loose block and proc arity is done.)
- A by-value `Proc` argument that the callee never stores is not freed by the
  callee (the F-25 by-value-argument ownership gap); for a shared Proc that is
  one owner never released, so the pair leaks.
- Proc owner counts are not atomic. `Proc[...]` is not `Send`, so a pair is
  never shared across threads today; a `Send` Proc needs an atomic count.
- `lambda` and `proc` followed by `{` or `do` always parse as closure literals,
  even when the program defines a method with that name.

## Pins

`tests/it/blocks_procs_lambdas.rs`, `tests/it/blocks_capture_moves.rs`,
`tests/it/borrow_check_pattern_moves.rs`; `drop_fixtures.rs`
(`blocks_share_string_capture`, `lambda_to_fn_param_no_leak`,
`block_capture_clone_moves_no_leak`, `capture_clone_moves_no_leak`,
`if_let_owned_option_no_leak`, `an_object_frees_the_lambda_in_its_field`,
`an_array_frees_the_lambdas_it_holds`, `a_hash_frees_the_procs_it_holds`,
`a_shared_proc_is_freed_once_by_its_last_owner`,
`a_block_called_from_a_for_loop_borrows_the_elements`);
`field_ownership::a_closure_field_read_is_a_borrow_shared_by_clone`; release-e2e
`2195_blocks_procs_lambdas_ownership`, `2197_array_each_closure_value`,
`2199_block_fewer_params` and `2201_closures_owned_by_fields_and_collections`;
`docs/errors/E1018.md`, `docs/errors/E1019.md`.
