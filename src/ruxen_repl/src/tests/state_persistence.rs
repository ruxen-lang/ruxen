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

#[test]
fn type_item_visible_from_later_input() {
    let outs = run_session(&[
        "class Point\n  x: Int\n  y: Int\n  def init(@x: Int, @y: Int) end\nend",
        "let p = Point.new(3, 4)",
        "p.x + p.y",
    ]);
    assert!(outs[2].contains("7"), "got: {:?}", outs);
}
