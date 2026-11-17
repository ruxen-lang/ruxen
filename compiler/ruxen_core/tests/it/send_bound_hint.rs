//! Q20 (gui-stack-v1-issues) — when constructing a `Mutex[T]` / `SharedSync[..]`
//! inside a generic type whose parameter isn't known to be `Send`, the
//! diagnostic should point at WHERE to add the bound (`[T: Send]` on the
//! enclosing class), not the misleading "add `include Send` to the class".

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn typeck_errors(source: &str) -> Vec<String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");
    typeck::type_check(&program)
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn send_bound_failure_on_generic_param_points_at_the_bound() {
    let source = "\
use std.sync.{Mutex, SharedSync}

class Cell[T]
  cell: SharedSync[Mutex[T]]
  def init(@initial: T)
    self.cell = SharedSync.new(Mutex.new(initial))
  end
end

def main
  puts \"ok\"
end
";
    let errors = typeck_errors(source);
    let helpful = errors
        .iter()
        .any(|m| m.contains("T: Send") && m.contains("declared"));
    assert!(
        helpful,
        "expected a hint to add `[T: Send]` where T is declared; got: {:#?}",
        errors
    );
}
