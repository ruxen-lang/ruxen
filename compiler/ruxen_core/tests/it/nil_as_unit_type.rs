//! Pin tests: `nil` is a synonym for `()` in type position.
//!
//! Spec: any place a type is parsed (`def f -> T`, generic args like
//! `Result[T, E]`, function-type return like `Fn() -> T`, `let x: T`)
//! must accept `nil` as a spelling of the unit type. The AST node
//! produced is IDENTICAL to the one produced by `()` — an empty
//! `TypeExpr::Tuple { elements: vec![], .. }`. Downstream consumers
//! (resolve, typeck, MIR, codegen) must observe no difference.
//!
//! Per `feedback_no_inline_rx_in_pin_tests`: source lives in
//! `.rx` fixtures under `tests/fixtures/ruxen/`, loaded via `rx(name)`.

use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Parse + typecheck a fixture, return only fatal-level diagnostics.
/// Warnings and inference gaps are tolerated — the pin is "the new
/// `nil`-in-type-position syntax does not introduce any new errors".
fn fatal_diags(name: &str) -> Vec<String> {
    let source = rx(name);
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn nil_as_return_type_parses_and_typechecks() {
    let errors = fatal_diags("nil_as_unit_type_basic");
    assert!(
        errors.is_empty(),
        "`def f -> nil` should parse + typecheck clean: {:?}",
        errors
    );
}

#[test]
fn nil_as_generic_arg_in_result_parses_and_typechecks() {
    let errors = fatal_diags("nil_as_unit_type_in_result");
    assert!(
        errors.is_empty(),
        "`Result[nil, String]` should parse + typecheck clean: {:?}",
        errors
    );
}

#[test]
fn nil_as_fn_return_type_in_param_position() {
    let errors = fatal_diags("nil_as_unit_type_in_fn_param");
    assert!(
        errors.is_empty(),
        "`any Fn() -> nil` parameter type should parse + typecheck clean: {:?}",
        errors
    );
}

#[test]
fn nil_in_both_generic_slots_with_let_annotation() {
    let errors = fatal_diags("nil_as_unit_type_mixed_let");
    assert!(
        errors.is_empty(),
        "`let x: Result[nil, nil] = Ok(nil)` should parse + typecheck clean: {:?}",
        errors
    );
}

/// Direct AST assertion: `nil` in return-type position lowers to the
/// SAME `TypeExpr::Tuple { elements: empty }` that `()` produces.
/// Guards against future refactors that bolt on a new dedicated
/// `TypeExpr::Nil` variant (which would silently bypass every existing
/// unit-type matcher in resolve/typeck/MIR/codegen).
#[test]
fn nil_in_type_position_lowers_to_empty_tuple_ast_node() {
    use ruxen_core::parser::ast::{TopLevelItem, TypeExpr};

    let source = rx("nil_as_unit_type_basic");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");

    let f = program
        .items
        .iter()
        .find_map(|item| match item {
            TopLevelItem::Function(func) if func.name == "f" => Some(func),
            _ => None,
        })
        .expect("function `f` should be present");

    let ret = f
        .return_type
        .as_ref()
        .expect("`def f -> nil` should have an explicit return type");

    match ret {
        TypeExpr::Tuple { elements, .. } => {
            assert!(
                elements.is_empty(),
                "`nil` in type position must produce an empty Tuple (== unit type `()`), \
                 got Tuple with {} element(s)",
                elements.len()
            );
        }
        other => panic!(
            "`nil` in type position must lower to TypeExpr::Tuple (the canonical unit type), \
             got {:?}",
            other
        ),
    }
}
