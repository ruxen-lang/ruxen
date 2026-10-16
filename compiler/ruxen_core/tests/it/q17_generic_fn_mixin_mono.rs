//! Q17: a generic method bound by a mixin that calls another generic method
//! across methods must fail with a clear lowering error, never call an
//! un-emitted opaque method or a bound-placeholder symbol.
//!
//! The compile-and-run cases live in the release-e2e fixtures 474, 655-659
//! and 2064.

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

/// The staged cross-method fixpoint is a hard error, never a call to an
/// un-emitted opaque method or a bound-placeholder symbol.
#[test]
fn generic_calling_generic_across_methods_errors_without_placeholder() {
    let source = "\
mixin Measured
  def measure -> Int
end

class Frame
  def inner[T: Measured](item: &var T) -> Int
    item.measure
  end

  def outer[U: Measured](item: &var U) -> Int
    self.inner(item)
  end
end

class Wide
  include Measured
  def measure -> Int
    7
  end
end

class Tall
  include Measured
  def measure -> Int
    9
  end
end

def main
  var frame = Frame.new
  var wide = Wide.new
  puts frame.outer(&var wide)
end
";
    let err = lower_error(source).expect("cross-method generic chain must fail clearly");
    assert!(
        err.contains("generic-calling-generic across methods"),
        "unexpected residual diagnostic: {err}"
    );
    assert!(
        !err.contains(": Measured_measure"),
        "bound placeholder leaked into residual diagnostic: {err}"
    );
}
