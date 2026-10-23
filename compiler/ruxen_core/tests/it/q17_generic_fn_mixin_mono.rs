//! The instance engine's limits: an expanding instantiation is a coded
//! error, never a hang.
//!
//! The compile-and-run cases live in the release-e2e fixtures 474, 655-659,
//! 2064 and 2247 (generic chains through methods, functions and classes).

use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn lower_error(source: &str) -> Option<String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let mut lowerer = Lowerer::new(&result.symbols);
    lowerer.lower_program(&result.program).err()
}

/// A generic body that instantiates itself with an ever larger type
/// (`Nest[T]#deeper` building `Nest[Array[T]]`) stops at the depth bound
/// with E0746 instead of instantiating forever.
#[test]
fn expanding_instantiation_is_e0746() {
    let source = "\
class Nest[T]
  v: T
  def init(@v: T)
  end
  def deeper -> Nest[Array[T]] where T: Clone
    Nest.new([@v.clone])
  end
end

n = Nest.new(1)
puts(n.deeper.v.size)
";
    let err = lower_error(source).expect("an expanding instantiation must fail");
    assert!(err.contains("error[E0746]"), "unexpected diagnostic: {err}");
}
