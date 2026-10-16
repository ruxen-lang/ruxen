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

/// F-18 — a function call is checked against the function it resolved to,
/// not looked up again by name. A module function call carries the mangled
/// name `Util_keep`, which names no definition, so the by-name lookup found
/// no signature and treated the borrowed argument as moved.
#[test]
fn module_fn_call_uses_its_resolved_signature() {
    let source = "\
class Shelf
  n: Int
  def init(@n: Int)
  end
end

module Util
  def keep(s: &Shelf) -> Int
    2
  end
end

def twice(s: Shelf) -> Int
  Util.keep(s)
  Util.keep(s)
end
";
    let errors = borrow_errors(source);
    assert!(
        errors.is_empty(),
        "`Util.keep` borrows `s`, so calling it twice must not move it; got: {:#?}",
        errors
    );
}

/// F-20 — the receiver mode comes from the method typeck selected, not
/// from its name. `Array#sort` is a `.rx` method that returns a sorted copy,
/// so calling it while a closure holds a shared borrow of the array is fine.
/// It used to be reported as a mutation (E1002) because `sort` is on the list
/// of mutating builtin names.
#[test]
fn non_mutating_builtin_method_named_like_a_mutator_is_not_a_mutation() {
    let source = "\
def main
  let v = [3, 1, 2]
  let size_of = { || v.size }
  let sorted = v.sort
  puts \"#{size_of.()} #{sorted.size}\"
end
";
    let errors = borrow_errors(source);
    assert!(
        errors.is_empty(),
        "`v.sort` returns a copy and must not count as a mutation of `v`; got: {:#?}",
        errors
    );
}
