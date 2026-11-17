# F-2 design, as built after the rethink: one fact per call result

Status: built 2026-11-17; the by-reference parameter seeding (I-77) added
2026-11-18. Owner rule: Ruby syntax on the surface, Rust's borrow mechanisms
underneath; infer first; no new markers. Follow-ups: `TODO.md` "F-2
follow-ups".

## The fact

Every call result has one `CallFact { borrows, carries }` (mir/nodes.rs), by
argument position, recorded as `View::Args` or `View::Holds` (a fresh value
that still owns its storage). Both the loan check and drop elaboration's
`borrows_of` read it; nothing else decides what a result borrows.

- borrows: the result points into, or holds loans on, those arguments' storage.
- carries: the result holds the loans those arguments already held.
- A by-value (`Move`) argument hands its loans to the result as before.

`h.remove(&key)` carries the table's references and borrows nothing of `key`,
so changing `key` afterwards is accepted; changing a string the table
referenced is still E1009.

## Declared half (hir/types/ty_helpers.rs `declared_result`)

- source: Rust's elision (`result_source`), unchanged.
- carries: a slot carries unless its declared type below its top reference
  holds nothing that could reach the result (`Ty::may_hold_reference`: a
  reference, a closure, a user class/struct/enum, a mixin value, or a type
  parameter that also occurs in the result). Type parameters are read before
  instance substitution. A callable slot's captures reach the result only
  through what it returns, unless the result can store a callable (a
  callable, mixin value, struct/enum or user class): `tap(f) -> Self`
  carries the receiver, not `f`; `then[U](f: Fn(&Self) -> U) -> U` carries `f`.
- keeps: `(from, into)` pairs. A callee writing `into` (`Mut`) keeps the
  references `from` passes only when `into`'s declared type may hold a
  reference to what `from` points at (`Ty::may_hold_reference_to`; a user
  class through its fields, `SymbolTable::fields_may_hold_reference_to`; a
  bare type parameter never). `def init(@r: &String)` on a class with a
  `&String` field keeps `r`; a class holding a `&var Canvas` keeps no
  `&String`. An undeclared callee applies the same rule to the argument
  types (`param_modes::call_stores`, the one rule both consumers follow).

## C helpers (drops/views.rs `record_default_views`, `Declared::Ffi`)

- A declared result that holds a reference borrows its elided source (Rust
  trusts an `extern` signature). Audit of the 17 std rows: plan Appendix.
- A value result borrows nothing only when provably new (`-> owned`, a Fresh
  row); otherwise every non-scalar argument (HEAD's rule). `reverse`, `clone`
  and `take` are therefore rejected while their copy is used (B1 makes them
  real copies). `SharedSync#clone` is declared `-> owned`.

## Body half (mir/loan_check/summary.rs)

The loan checker's own transfer, run in summary mode: each parameter `p`
starts with a `LoanOn::Held(p)` marker (conflict checks ignore it) standing
for the references its value already holds. A by-value parameter holds it
itself. A by-reference (`Read` or `Mut`) parameter is what Rust's `&'a T` is:
it denotes a phantom caller place (a local past the body's own) that holds
`Held(p)`, and it holds a place loan on that place, as the caller's `&x`
argument does. So a result that stores or returns the reference
(`def wrap(s: &String) -> Holder; Holder.new(s)`) borrows `p` and the caller
keeps its `&x` loan, while a result that keeps only what the place stores
(`h.remove(k)` through `h: &var Hash`) carries `p`. At each `Return` the
returned local borrows `p` when a place it denotes is rooted at `p` or its
phantom, or it holds a place loan rooted there, and carries `p` when it
holds `Held(p)`. A body with no recorded parameter modes gets every non-scalar
parameter in both halves. A body callee's view is today's body rule (elided
source, `returned_params`, `call_result_may_borrow_arg`) plus its summary.

## The joint fixpoint

`summary::fixpoint`, run from `lower/mod.rs` before the loan check and drops:
each round resets a body to lowering's own views, records its call views from
the facts so far (`drops::record_default_views`), reruns the transfer, and
unions body facts and closure loan effects with the previous round. Only the
callers of a grown fact rerun (and the bodies that build a closure pair when
an effect grew). Sets only grow and are finite, so it ends with no round cap
(`MirProgram::summary_rounds`; a chain of three and a recursive pair take at
most five). `check_program` starts from the published loan effects; its
replay adds the access effects.

## Not done here

Body versus signature check (Rust's), B1 owned C results, making
`borrows_of` read the transfer: TODO.md "F-2 follow-ups".
