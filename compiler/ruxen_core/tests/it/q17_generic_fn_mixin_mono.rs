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

/// A body that doubles its type on every step (`Nest[T]` building
/// `Nest[(T, T)]`) is bounded by size, not depth: the size check runs before
/// every instance is recorded, so it ends within a few steps, where a depth
/// bound let the arguments reach `2^depth` nodes. The message names the
/// bound and the template's own line.
#[test]
fn doubling_instantiation_stops_at_the_size_bound() {
    let source = "\
# Two lines before the template, so its location is not the file start.

class Nest[T]
  v: T
  def init(@v: T)
  end
  def deeper -> Nest[(T, T)] where T: Clone
    Nest.new((@v.clone, @v.clone))
  end
end

n = Nest.new(1)
puts(n.deeper.v.0)
";
    let err = lower_error(source).expect("a doubling instantiation must fail");
    assert!(
        err.contains("error[E0746]: `Nest`"),
        "unexpected diagnostic: {err}"
    );
    let bound = format!(
        "more than {} type nodes",
        ruxen_core::hir::types::MAX_TYPE_NODES
    );
    assert!(err.contains(&bound), "the size bound must stop it: {err}");
    assert!(
        err.contains("(at 3:1)"),
        "E0746 points at the template: {err}"
    );
}

/// A wide fan-out (`Nest[T]` building a four-wide tuple of itself) stops
/// at the same size bound.
#[test]
fn wide_fan_out_instantiation_is_e0746() {
    let source = "\
class Nest[T]
  v: T
  def init(@v: T)
  end
  def wider -> Nest[(T, T, T, T)] where T: Clone
    Nest.new((@v.clone, @v.clone, @v.clone, @v.clone))
  end
end

n = Nest.new(1)
puts(n.wider.v.0)
";
    let err = lower_error(source).expect("a fan-out instantiation must fail");
    assert!(err.contains("error[E0746]"), "unexpected diagnostic: {err}");
}

/// A class holding a field of itself over a larger type (`Boom[T]` with a
/// `Boom[(T, T)]` field) is polymorphic recursion: E0746, where the derive
/// check used to expand it without end.
#[test]
fn self_field_over_a_larger_type_is_e0746() {
    let source = "\
class Boom[T]
  v: T
  next: Option[Boom[(T, T)]]

  def init(@v: T)
    @next = nil
  end
end

b = Boom.new(1)
puts(b.v)
";
    let err = lower_error(source).expect("polymorphic recursion must fail");
    assert!(err.contains("error[E0746]"), "unexpected diagnostic: {err}");
}

/// I-86: each top-level item merged from the standard library records its
/// package, so a diagnostic about a stdlib template names where it lives;
/// a method's origin is its owner's, and the user's own items have none.
#[test]
fn stdlib_items_record_their_package() {
    use ruxen_core::driver;
    let program = driver::parse("class Mine\n  v: Int\n  def get -> Int\n    @v\n  end\nend\n")
        .expect("parse");
    let frontend = driver::check(&program, &driver::Options::default());
    let symbols = frontend.symbols();
    let hash = symbols.type_def("Hash").expect("Hash").id;
    assert_eq!(symbols.origin(hash), Some("map"));
    let keys = symbols
        .iter()
        .find(|d| d.name == "keys" && matches!(d.kind, ruxen_core::resolve::symbols::DefKind::Method { parent, .. } if parent == hash))
        .expect("Hash#keys")
        .id;
    assert_eq!(symbols.origin(keys), Some("map"));
    let mine = symbols.type_def("Mine").expect("Mine").id;
    assert_eq!(symbols.origin(mine), None);
}
