//! #06.93 Phase 2 pin tests: `::Name` root anchor at type position.
//!
//! Verifies (a) the parser accepts a leading `::` before a type
//! identifier and produces `TypePath.rooted = true`, (b) the
//! resolver's `resolve_type_path` bypasses the
//! `scopes.lookup_type` fallback for rooted paths (which is the
//! shadowing-sensitive lookup site).
//!
//! Phase 2 establishes the SYNTAX + plumbing — the discriminating
//! semantic (rooted resolves to a different DefId than the inner
//! one) only becomes user-observable once Phase 4's inner-first
//! shadowing changes scope semantics. For Phase 2 we pin that the
//! parser + resolver accept the form without error.
//!
//! Per `feedback_no_inline_rx_in_pin_tests`: every Ruxen sample
//! lives in a `.rx` fixture under `tests/fixtures/ruxen/`,
//! loaded via the `rx(name)` helper.

use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::resolve::Resolver;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn resolve_errors(name: &str) -> Vec<String> {
    let source = rx(name);
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let resolver = Resolver::new();
    let result = resolver.resolve(&program);
    result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn root_anchor_parses() {
    let errors = resolve_errors("root_anchor_parses");
    assert!(
        errors.is_empty(),
        "::Foo at parameter-type position should parse + resolve clean: {:?}",
        errors
    );
}

#[test]
fn root_anchor_resolves_globally() {
    let errors = resolve_errors("root_anchor_resolves_globally");
    assert!(
        errors.is_empty(),
        "top-level + module-inner + root-anchored should all resolve: {:?}",
        errors
    );
}

/// Direct AST assertion: the parser must mark `::Foo` as rooted on
/// `TypePath`. Catches regressions where the parser stops setting
/// the flag (e.g. if a refactor reverts the change).
#[test]
fn parser_sets_rooted_flag_on_leading_double_colon() {
    use ruxen_core::parser::ast::{TopLevelItem, TypeExpr};

    let source = rx("root_anchor_parses");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");

    let take_fn = program
        .items
        .iter()
        .find_map(|item| match item {
            TopLevelItem::Function(f) if f.name == "take_rooted" => Some(f),
            _ => None,
        })
        .expect("take_rooted function should be present");

    let param = take_fn
        .params
        .first()
        .expect("take_rooted has one parameter");

    match &param.type_expr {
        TypeExpr::Named(path) => {
            assert!(
                path.rooted,
                "TypePath for `::Foo` parameter should have rooted=true; got rooted={}, segments={:?}",
                path.rooted,
                path.segments
            );
            assert_eq!(path.segments, vec!["Foo".to_string()]);
        }
        other => panic!("expected TypeExpr::Named for `::Foo`, got {:?}", other),
    }
}
