//! Golden tests pinning the user-visible REPL state-persistence
//! contract. These were added before the `all_statements`-replay
//! removal so the refactor can't silently regress the surface
//! semantics.
//!
//! Each test feeds a sequence of inputs to a fresh `ReplSession` via
//! the real `eval_input` entry point and asserts on the displayed
//! output of the final input.

use crate::eval::{eval_input, EvalResult};
use crate::session::ReplSession;

/// Feed a sequence of inputs to a fresh session, capturing the
/// real-stdout side of each `EvalResult`. Used as the golden harness
/// for every test in this file.
pub(crate) fn run_session(inputs: &[&str]) -> Vec<String> {
    let mut session = ReplSession::new().expect("session");
    inputs
        .iter()
        .map(|inp| match eval_input(&mut session, inp) {
            EvalResult::Ok(Some(s)) => s,
            EvalResult::Ok(None) => String::new(),
            EvalResult::Command(s) => s,
            EvalResult::Quit => String::new(),
            EvalResult::Incomplete => {
                panic!("input {:?} → unexpected Incomplete", inp)
            }
            EvalResult::Error(e) => panic!("input {:?} → {}", inp, e),
        })
        .collect()
}

#[test]
fn let_binding_survives_next_input() {
    let outs = run_session(&["let x = 41", "x + 1"]);
    assert!(outs[1].contains("42"), "got: {:?}", outs);
}

#[test]
fn mutation_persists_across_inputs() {
    let outs = run_session(&[
        "let mut counter = 0",
        "counter = counter + 1",
        "counter = counter + 1",
        "counter",
    ]);
    assert!(outs[3].contains("2"), "got: {:?}", outs);
}

#[test]
fn def_callable_from_later_input() {
    let outs = run_session(&[
        "def double(n: Int) -> Int; n * 2; end",
        "double(21)",
    ]);
    assert!(outs[1].contains("42"), "got: {:?}", outs);
}

/// With Task 1.2's synthetic slot-load prefix, a session Int
/// variable's value comes from the slot at every input — even
/// when the all_statements replay path is also active (the
/// prefix-defined binding shadows the replayed one). Phase 3
/// will remove the replay; this test must keep passing.
#[test]
fn int_var_read_from_slot_persists() {
    let outs = run_session(&[
        "let answer = 42",
        "answer",
    ]);
    assert!(outs[1].contains("42"), "got: {:?}", outs);
}

/// After a successful `let` binding, the session must own a
/// `VarSlot` for the name with type `Ty::Int`. Phase 1
/// (`register_var` wiring) — the synthetic prefix that consumes
/// these `VarSlot`s in `build_program` is meaningless without
/// them being populated in the first place.
#[test]
fn int_let_allocates_var_slot() {
    use crate::eval::eval_input;
    use crate::session::ReplSession;
    use ruxen_core::hir::types::Ty;

    let mut session = ReplSession::new().expect("session");
    match eval_input(&mut session, "let answer = 42") {
        crate::eval::EvalResult::Ok(_) => {}
        other => panic!("eval failed: {:?}", match other {
            crate::eval::EvalResult::Error(e) => e,
            _ => "non-Ok, non-Error result".into(),
        }),
    }
    let slot = session
        .find_var_slot("answer")
        .expect("expected a slot for `answer` after `let answer = 42`");
    assert_eq!(slot.name, "answer");
    assert_eq!(slot.ty, Ty::Int, "expected Ty::Int, got {:?}", slot.ty);
}

#[test]
fn type_item_visible_from_later_input() {
    let outs = run_session(&[
        "class Point\n  x: Int\n  y: Int\n  def init(@x: Int, @y: Int) end\nend",
        "let p = Point.new(3, 4)",
        "p.x + p.y",
    ]);
    assert!(outs[2].contains("7"), "got: {:?}", outs);
}
