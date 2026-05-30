//! Pins the Cranelift JIT verifier failure when user input contains
//! a `return` statement inside a control-flow block at top level.
//! The wrapper's tail-preservation transform (Task 1.3 in the REPL
//! state refactor) produces a function whose declared return type
//! comes from the trailing expression; an embedded `return` exits
//! with a possibly-different type and the verifier rejects it.
//!
//! These tests must PASS after Phase 1 of the unblock plan lands
//! (a skip-the-transform-when-input-contains-return guard).
//!
//! Each test feeds a single REPL input that is a single statement
//! (so the REPL's `parse_repl_input` consumes the whole input as
//! one `ReplInput`). `run_session` panics on `EvalResult::Error`,
//! so the contract a passing test pins is: the wrapper for an
//! input embedding `return` compiles cleanly through the Cranelift
//! verifier and runs without error.

use super::state_persistence::run_session;

#[test]
fn return_inside_if_at_top_level_compiles() {
    // Two prior session vars (handle + sentinel) so the slot prefix
    // / suffix actually fire — mirrors 727's state at the failing
    // input (`if handle == 0; puts ...; return; end`).
    let outs = run_session(&[
        "let handle = 5",
        "let sentinel = 7",
        "if handle == 0\n  puts \"spawn_fail\"\n  return\nend",
    ]);
    // handle != 0, so the if branch is skipped. No assertion on
    // stdout: the contract is that the wrapper compiles cleanly
    // (no verifier-error panic via run_session).
    let _ = outs;
}

#[test]
fn return_inside_if_then_no_else_compiles() {
    let outs = run_session(&["let x = 0", "if x == 0\n  return\nend"]);
    // x == 0 is true, the bare `return` exits the wrapper. The
    // contract is that the wrapper passes the Cranelift verifier
    // even though the if has no else branch.
    let _ = outs;
}

#[test]
fn return_inside_else_arm_compiles() {
    let outs = run_session(&[
        "let x = 5",
        "if x == 0\n  puts \"zero\"\nelse\n  return\nend",
    ]);
    // x != 0, the else branch's `return` exits the wrapper.
    // Pins that the if/else shape with a bare-return arm doesn't
    // break the tail-preservation skip guard either.
    let _ = outs;
}

/// The replayed bare `return` from a prior input must not collide
/// with the natural tail of the wrapper for a subsequent let-binding
/// input. Specifically: input 1 binds an Int; input 2 uses `if…return…end`
/// (no else); input 3 does `let ok = some_int_call`. The wrapper for
/// input 3 has to swallow the replayed return cleanly — by emitting
/// `()` as the tail instead of the natural `ok`-name tail.
///
/// Pre-Phase 2 this crashed with
/// `Compilation(Verifier(... "arguments of return must match function
/// signature"))`; `run_session` would panic on `EvalResult::Error`
/// before any assertion ran. So the contract pinned here is purely
/// "the wrapper compiles cleanly". The display value (`outs[3]`,
/// `outs[4]`) is documented to be empty when the wrapper body
/// contains a `return` — matches compile-and-run semantics
/// (`def main; …; return; end` returns Unit) and is the explicit
/// trade-off of the Phase 2 design.
#[test]
fn let_after_replayed_return_compiles() {
    let outs = run_session(&[
        "def maybe_fail(x: Int) -> Int; if x == 0; 1; else; x; end; end",
        "let n = 5",
        "if n == 0\n  puts \"zero\"\n  return\nend",
        "let ok = maybe_fail(n)",
        "ok",
    ]);
    // 5 outputs, none of them an error panic (run_session would
    // have already panicked on EvalResult::Error). Display tails
    // are intentionally suppressed for the post-return inputs.
    assert_eq!(outs.len(), 5, "got: {:?}", outs);
}

/// Variant: a side-effecting `puts` after a return-containing
/// replay. The wrapper for the `puts` input must accept the
/// replayed bare return.
#[test]
fn puts_after_replayed_return_compiles() {
    let outs = run_session(&[
        "let n = 5",
        "if n == 0\n  puts \"zero\"\n  return\nend",
        "puts \"reached\"",
    ]);
    assert!(outs[2].contains("reached"), "got: {:?}", outs);
}

/// Pins the 727 hang fix. A slot-backed Int let (here, `handle`) is
/// replayed on every subsequent input — but its replay would shadow
/// the slot-loaded binding with a freshly-evaluated RHS. For
/// non-idempotent RHS (here, simulated with a side-effecting marker)
/// the rebind would diverge from the slot value. The fix filters
/// slot-backed lets out of the replay stream so the slot load wins.
#[test]
fn slot_backed_let_does_not_shadow_via_replay() {
    let outs = run_session(&[
        // First-time execution stores 42 in the slot for `n`.
        "let n = 42",
        // Subsequent inputs' replay should NOT re-execute `let n = 42`
        // (which would also work in this trivial case, but pins the
        // contract). The if-block conditional fires on the slot value.
        "if n == 42\n  puts \"slot_kept\"\nelse\n  puts \"slot_lost\"\nend",
    ]);
    assert!(outs[1].contains("slot_kept"), "got: {:?}", outs);
}

/// Specifically pins the 727 shape: a let whose RHS is a method-call
/// or function-call (NOT a literal). Without the filter, the
/// replayed call re-runs every input and the user code that depends
/// on the original return value sees a different value.
#[test]
fn slot_backed_let_with_call_rhs_does_not_replay() {
    let outs = run_session(&[
        // Define a function that returns a different value each call
        // (we can't easily get truly stateful Ruxen here, so use a
        // pure function and test the simpler invariant: the slot
        // load must be the source of truth).
        "def make_handle -> Int; 99; end",
        "let handle = make_handle()",
        "if handle == 99\n  puts \"original_handle\"\nelse\n  puts \"rebound_handle\"\nend",
    ]);
    assert!(outs[2].contains("original_handle"), "got: {:?}", outs);
}
