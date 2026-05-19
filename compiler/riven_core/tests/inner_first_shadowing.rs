//! #06.93 Phase 4 pin tests: inner-first scope shadowing inside
//! module bodies + invisibility of module-inner types from outer
//! scopes.
//!
//! Two complementary properties:
//!
//! 1. **Inner-first**: inside `module M`, an un-qualified `Foo`
//!    references `M.Foo` if one exists, regardless of whether a
//!    top-level `Foo` also exists. (Phase 4 enables this by
//!    inserting module-inner type names into the module's own
//!    scope frame, not the global one.)
//!
//! 2. **Invisible outside**: at the top level, an un-qualified
//!    `Inner` does NOT resolve to a module-nested `M.Inner` —
//!    only the qualified `M.Inner` path works. (Phase 4 stops
//!    inserting the un-qualified key into `type_registry` for
//!    module-nested types.)
//!
//! Per `feedback_no_inline_rvn_in_pin_tests`: every Riven sample
//! lives in a `.rvn` fixture, loaded via `rvn(name)`.

use riven_core::diagnostics::DiagnosticLevel;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::resolve::Resolver;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn resolve_errors(name: &str) -> Vec<String> {
    let source = rvn(name);
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
fn module_inner_class_is_invisible_at_top_level_via_unqualified_name() {
    // After Phase 4, top-level `_x: Inner` does NOT resolve to
    // `Outer.Inner`. The resolver must emit "undefined type" or
    // similar.
    let errors = resolve_errors("module_inner_invisible_outside");
    assert!(
        !errors.is_empty(),
        "top-level un-qualified `Inner` should NOT resolve to module-nested Outer.Inner after Phase 4"
    );
    let has_unknown = errors
        .iter()
        .any(|e| e.contains("Inner") && (e.contains("undefined") || e.contains("unknown")));
    assert!(
        has_unknown,
        "expected an 'undefined/unknown type Inner' diagnostic; got: {:?}",
        errors
    );
}
