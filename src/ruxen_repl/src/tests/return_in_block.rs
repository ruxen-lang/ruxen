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
    let outs = run_session(&[
        "let x = 0",
        "if x == 0\n  return\nend",
    ]);
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
