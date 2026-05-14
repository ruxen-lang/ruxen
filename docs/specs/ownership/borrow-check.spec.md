# Spec — Borrow check (move / ref / mut-ref)

**Source docs:**
[docs/requirements/tier1_04_drop_copy_clone.md](../../requirements/tier1_04_drop_copy_clone.md),
[docs/dev/](../../dev/) (borrow-check notes).

**Status:** shipped Phase 2 #02 (Rust-style borrow check on MIR).

Riven's borrow checker runs after typeck on the MIR.  It rejects
use-after-move, mutable aliasing, and dangling borrows.  This spec
captures the **observable** rejection envelope; cross-reference
`docs/dev/` for the implementation notes.

---

## B1 — Sample-program acceptance baseline

The `borrow_check_sample.rvn` fixture is a canonical mid-size program
that exercises move semantics, immutable borrowing, and mutable
borrowing across function boundaries.  Borrow check accepts it
without diagnostics.

This serves as a regression baseline: changes to the borrow checker
must keep this program accepted.

## B2 — Use-after-move on `String` argument is rejected

**Given** a function `f(s: String)` that takes ownership of `s`, and
a caller `let x = String.from("hi"); f(x); f(x);`
**Then** the second `f(x)` is rejected as use-after-move.

(Same behaviour applies to any non-`Copy` owned type — `Vec`,
`HashMap`, etc.)

## B3 — `into_*` methods consume the receiver

`String::into_bytes`, `Vec::into_iter`, and any other method whose
signature takes `self` (not `&self` / `&mut self`) consumes the
receiver.  Subsequent use of the original binding is rejected.

## B4 — `&mut T` does not coerce in ways that would alias

Covered by [variance.spec.md](../mixins/variance.spec.md) B1 / B3.
Mentioned here because the borrow checker enforces the rule at
function boundaries; the variance checker enforces it at coercion
sites.  Both must agree.

---

## Pin tests

| Behaviour | Test fn                                       | File                            |
|-----------|-----------------------------------------------|---------------------------------|
| B1        | `sample_program_borrow_checks`                | `borrow_check_sample.rs`        |
| B2        | `use_after_move_on_string_argument_is_rejected` | `stdlib_string_negatives.rs`  |
| B3        | `use_after_into_bytes_is_rejected`            | `stdlib_string_negatives.rs`    |

---

## Gaps

- The borrow checker has many internal rules (NLL-style two-phase
  borrows, conditional moves, partial moves through struct fields)
  that today are pinned by unit tests inside `crates/riven-core/src/`
  rather than at the integration boundary.  Listing them here would
  duplicate the unit suite — point readers to
  `crates/riven-core/src/borrow_check/tests.rs` and only surface
  observable rejection-envelope behaviours at the integration layer.

## Out of scope (v2)

- Polonius-style next-gen borrow checker (the current implementation
  is NLL-equivalent).
- User-controllable lifetimes beyond the surface in
  `tests/release-e2e/cases/102_lifetime_explicit.rvn`.
- `Pin[T]` and self-referential structures.
- Reborrowing across closure captures (closures currently take
  ownership of captures unless marked otherwise).
