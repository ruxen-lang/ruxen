//! Borrow-check regression pins for reference reborrow + move-capture
//! (gui-stack-v1-issues Q12 / Q4). These exercise the *diagnostics* pass
//! (the e2e harness does not borrow-check), so the program is parsed and
//! type-checked but not run — an inline source string is appropriate here.

use ruxen_core::borrow_check;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn borrow_errors(source: &str) -> Vec<String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");
    let type_result = typeck::type_check(&program);
    borrow_check::borrow_check(&type_result.program, &type_result.symbols)
        .iter()
        .map(|e| e.to_string())
        .collect()
}

/// Q12 — passing a `&var T` reference parameter as an argument is an
/// implicit REBORROW, not a move, so it can be passed to several calls.
#[test]
fn reference_param_passed_twice_is_reborrow_not_move() {
    let source = "\
class Ui
  n: Int
  def init
    self.n = 0
  end
end

class State
  v: Int
  def init(@v: Int)
  end
  def get(u: &var Ui) -> Int
    self.v
  end
end

def use_twice(a: State, b: State, u: &var Ui) -> Int
  if a.get(u) > 0
    a.get(u)
  else
    b.get(u)
  end
end
";
    let errors = borrow_errors(source);
    assert!(
        errors.is_empty(),
        "passing a `&var` reference param as an arg twice must not move it; got: {:#?}",
        errors
    );
}

/// Q4 — a `move` closure capturing a non-Copy class value owns its captured
/// copy, so USING that value inside the body is valid (the binding is moved
/// only for the OUTER scope after the closure, not inside it).
#[test]
fn move_closure_body_can_use_captured_non_copy_value() {
    let source = "\
class Handle
  v: Int
  def init(@v: Int)
  end
  def get -> Int
    self.v
  end
end

def main
  let h = Handle.new(42)
  let f = move { || puts \"v=#{h.get}\" }
  f.()
end
";
    let errors = borrow_errors(source);
    assert!(
        errors.is_empty(),
        "a move closure's body may use the value it captured; got: {:#?}",
        errors
    );
}

/// Q4 (regression guard) — the move-capture must STILL invalidate the OUTER
/// binding: using the captured value AFTER the closure is a real
/// use-after-move. The Q4 fix only frees up the body, not the outer scope.
#[test]
fn value_used_after_move_closure_still_errors() {
    let source = "\
class Handle
  v: Int
  def init(@v: Int)
  end
  def get -> Int
    self.v
  end
end

def main
  let h = Handle.new(42)
  let f = move { || puts \"v=#{h.get}\" }
  f.()
  puts \"#{h.get}\"
end
";
    let errors = borrow_errors(source);
    assert!(
        errors.iter().any(|e| e.contains("E1001")),
        "using a move-captured value after the closure must still be E1001; got: {:#?}",
        errors
    );
}
