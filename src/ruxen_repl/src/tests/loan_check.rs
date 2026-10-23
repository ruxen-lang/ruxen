//! The REPL runs the same front end as `ruxen build`, MIR loan check
//! included, so an input `ruxen build` rejects is rejected here too.

use crate::eval::{eval_input, EvalResult};
use crate::session::ReplSession;

/// I-30: the index operand `reset(u)` mutates `u.name` while the array
/// literal still holds a borrow of it.
#[test]
fn index_operand_mutating_a_borrowed_field_is_e1002() {
    let mut session = ReplSession::new().expect("session");
    let accepted = [
        "class U\n  name: String\n  def init(@name: String)\n  end\nend",
        "def reset(u: &var U) -> Int\n  u.name = \"reset-a-long-enough-string\"\n  1\nend",
        "u = U.new(\"old\")",
    ];
    for input in accepted {
        if let EvalResult::Error(e) = eval_input(&mut session, input) {
            panic!("input {input:?} -> {e}");
        }
    }
    match eval_input(&mut session, "puts([u.name][reset(u)])") {
        EvalResult::Error(e) => assert!(e.contains("E1002"), "got: {e}"),
        _ => panic!("the I-30 shape must be rejected"),
    }
}
