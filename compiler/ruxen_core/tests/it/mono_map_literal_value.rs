//! Phase 6 Task 6 characterization: a generic class instantiated ONLY
//! inside a map-literal value expression must still be monomorphized.
//!
//! `walk_tys_in_expr` (mir/lower/monomorphize.rs) had a trailing `_ => {}`
//! that silently dropped `HirExprKind::MapLiteral` — so the value
//! expression's types (here `Box[Int]` from `Box.new(5)`) were never
//! recorded as a monomorphization target. The let binding's own type is
//! `Map[Int, Box[Int]]`, and the instance collector does NOT recurse into
//! a `Map`'s generic args, so the map-literal value expr was the only path
//! to `Box[Int]`. Closing the catch-all makes the `Box[Int]` specialization
//! get emitted; without it codegen emits an unresolved `Box_get` symbol.

use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn mono_function_names(source: &str) -> Vec<String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);

    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");
    mir.functions.iter().map(|f| f.name.clone()).collect()
}

/// The generic class `Box[T]` appears only as a map-literal value
/// (`{ 1 => Box.new(5) }`). The mono pass must specialize `Box[Int]`,
/// emitting a `Box_Int`-mangled method, reachable only by walking the
/// map-literal value expression.
#[test]
fn generic_class_in_map_literal_value_is_monomorphized() {
    let src = "\
class Box[T]
  v: T
  def init(@v: T)
  end
  def get -> T
    self.v
  end
end

def main
  let m = { 1 => Box.new(5) }
end
";
    let names = mono_function_names(src);
    // The monomorphized Box[Int] method is mangled with the concrete arg
    // (`Box__mono__Int_get`), distinct from the generic base `Box_get`.
    // Without recursing into the map-literal value, no `__mono__` Box
    // specialization is emitted at all.
    assert!(
        names
            .iter()
            .any(|n| n.contains("mono") && n.contains("get")),
        "expected a monomorphized Box.get specialization; got functions: {:?}",
        names
    );
}
