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

/// F-21: a generic class body that dispatches on its mixin-bound `T` is
/// dropped when only monomorphised copies are called (e2e 2151). When a live
/// call still reaches the opaque body (here `Outer[T]` holds a `Holder[T]`,
/// and `Outer`'s instantiation is not recorded), lowering fails and names
/// that caller instead of letting an abstract `?T_…` callee reach codegen.
#[test]
fn reachable_unmonomorphised_generic_class_body_errors() {
    let source = "\
mixin Showable
  def to_display -> String
end

class Apple
  n: Int
  def init(@n: Int)
  end
  include Showable
  def to_display -> String
    \"apple\"
  end
end

class Pear
  n: Int
  def init(@n: Int)
  end
  include Showable
  def to_display -> String
    \"pear\"
  end
end

class Holder[T: Showable]
  item: T
  def init(@item: T)
  end
  def show -> String
    self.item.to_display
  end
end

class Outer[T: Showable]
  h: Holder[T]
  def init(@h: Holder[T])
  end
  def run -> String
    self.h.show
  end
end

def main
  let o = Outer.new(Holder.new(Apple.new(1)))
  puts o.run
end
";
    let err = lower_error(source).expect("a live opaque generic body must fail clearly");
    assert!(
        err.contains("cannot lower `Holder_show` (called from `Outer_run`)"),
        "unexpected diagnostic: {err}"
    );
}
