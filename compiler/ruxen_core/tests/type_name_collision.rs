//! Q14 (gui-stack-v1-issues) — a user type whose name collides with an
//! auto-loaded stdlib type (`Signal`, `Runner`, …) used to fail late at
//! codegen with `DuplicateDefinition("Signal_clone")`. It now produces a
//! clear resolve-time E0727 with a rename hint.

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
        .map(|d| format!("{}|{}", d.code.clone().unwrap_or_default(), d.message))
        .collect()
}

#[test]
fn user_class_colliding_with_stdlib_type_is_e0727() {
    // `Signal` is exported by std.sync and auto-loaded into every program.
    let source = "\
class Signal[T: Send]
  cell: Int
  def init(@cell: Int)
  end
end

def main
  puts \"hi\"
end
";
    let errors = typeck_errors(source);
    let flagged = errors
        .iter()
        .any(|m| m.starts_with("E0727|") && m.contains("Signal") && m.contains("collides"));
    assert!(
        flagged,
        "expected E0727 for the `Signal` name collision; got: {:#?}",
        errors
    );
}
