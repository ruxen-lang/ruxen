//! Q19 (gui-stack-v1-issues) — `Stdin.new` (a class with no constructor: no
//! `init`, no fields) should produce a clear typeck error pointing at the
//! free function, not a late "undefined _Stdin_init" linker error.

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
fn constructor_less_class_new_errors_at_typeck() {
    // A field-less, init-less class stands in for the FFI handle types
    // (Stdin/Stdout/Stderr) — `.new` on it has no `_init` symbol.
    let source = "\
class Handle
  def use_it -> Int
    0
  end
end

def main
  let h = Handle.new
  puts \"#{h.use_it}\"
end
";
    let errors = typeck_errors(source);
    let flagged = errors
        .iter()
        .any(|m| m.contains("Handle") && m.contains("no constructor"));
    assert!(
        flagged,
        "expected a 'no constructor' typeck error for `Handle.new`; got: {:#?}",
        errors
    );
}
