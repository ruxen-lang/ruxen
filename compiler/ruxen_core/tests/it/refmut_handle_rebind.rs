//! E1020: assigning to a `&var` parameter whose type is passed as its handle
//! word (class, struct, enum, array, hash, closure, type parameter) is a
//! borrow-check error, because the rebinding would never reach the caller.
//! A slot-addressed `&var` (String, scalar) may be assigned, and a handle may
//! be mutated through. Diagnostics only, so the source is checked, not run;
//! the runtime behaviour of the allowed forms is pinned by e2e `2216`.

use ruxen_core::borrow_check;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn borrow_codes(source: &str) -> Vec<String> {
    let tokens = Lexer::new(source).tokenize().expect("lexer failed");
    let program = Parser::new(tokens).parse().expect("parser failed");
    let result = typeck::type_check(&program);
    let type_errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(type_errors.is_empty(), "type errors: {type_errors:?}");
    borrow_check::borrow_check(&result.program, &result.symbols)
        .iter()
        .map(|e| e.code.code_str().to_string())
        .collect()
}

const POINT: &str = "\
class Point
  x: Int
  def init(@x: Int)
  end
end
";

#[test]
fn rebinding_a_class_var_param_is_e1020() {
    let source = format!("{POINT}\ndef reset(p: &var Point) -> nil\n  p = Point.new(0)\nend\n");
    assert_eq!(borrow_codes(&source), vec!["E1020"]);
}

#[test]
fn rebinding_an_array_var_param_is_e1020_for_every_assignment_form() {
    let source = "\
def replace(items: &var Array[Int]) -> nil
  items = [1, 2]
  items += [3]
end
";
    assert_eq!(borrow_codes(source), vec!["E1020", "E1020"]);
}

#[test]
fn rebinding_a_type_param_var_param_is_e1020() {
    let source = "\
def put[T](slot: &var T, v: T) -> nil
  slot = v
end
";
    assert_eq!(borrow_codes(source), vec!["E1020"]);
}

#[test]
fn e1020_help_says_how_to_fix_it() {
    let source = format!("{POINT}\ndef reset(p: &var Point) -> nil\n  p = Point.new(0)\nend\n");
    let tokens = Lexer::new(&source).tokenize().expect("lexer failed");
    let program = Parser::new(tokens).parse().expect("parser failed");
    let result = typeck::type_check(&program);
    let errors = borrow_check::borrow_check(&result.program, &result.symbols);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(
        errors[0].help,
        vec!["return the new value, or wrap it in a field".to_string()]
    );
}

#[test]
fn slot_var_params_and_mutation_through_a_handle_are_allowed() {
    let source = format!(
        "{POINT}
def bump(n: &var Int, s: &var String) -> nil
  n += 1
  s = \"new\"
end

def shift(p: &var Point, items: &var Array[Int]) -> nil
  p.x = 3
  items.push(4)
end
"
    );
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
}
