//! Q13 (gui-stack-v1-issues) — `obj.method?` lexes the trailing `?` into the
//! member name (Ruby predicate names like `empty?` are legal). When such a
//! name resolves to nothing, the diagnostic should hint that the try-operator
//! is `foo()?` and safe navigation is `&.` (Ruby), not a trailing `?`.
//! Diagnostics-only pass, so the program is parsed + type-checked (not run).

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
fn unknown_predicate_named_member_hints_try_and_safe_nav() {
    let source = "\
class Canvas
  n: Int
  def init
    self.n = 0
  end
end

def main
  let cv = Canvas.new
  cv.begin_frame?
end
";
    let errors = typeck_errors(source);
    let hinted = errors
        .iter()
        .any(|m| m.contains("begin_frame?") && m.contains("begin_frame()?") && m.contains("&."));
    assert!(
        hinted,
        "expected a predicate/try-operator hint on `begin_frame?`; got: {:#?}",
        errors
    );
}
