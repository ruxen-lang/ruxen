//! #06.93 Phase 1 pin test: classes (and other types) declared
//! inside a `module Foo` get a qualified `Foo.ClassName` entry in
//! `type_registry` alongside the existing un-qualified entry, all
//! pointing at the same `DefId`.
//!
//! Tested through the user-observable signal: `Foo.ClassName` at a
//! type position must resolve without error. Without Phase 1's
//! qualified-key insertion, the resolver would emit
//! "unknown type `Foo.ClassName`" when typeck encounters such an
//! annotation.
//!
//! Phase 1 is purely additive — no scope-shadowing change, no
//! expression-resolution change. Method-call dispatch via
//! `Foo.ClassName.method(...)` is Phase 3's job; inner-first
//! shadowing is Phase 4's.
//!
//! Per `feedback_no_inline_rvn_in_pin_tests`: every Riven source
//! sample lives in a `.rvn` fixture under `tests/fixtures/riven/`
//! and is loaded via the `rvn(name)` helper.

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
fn module_class_resolves_via_qualified_type_path() {
    let errors = resolve_errors("module_class_qualified_param_type");
    assert!(
        errors.is_empty(),
        "Outer.Inner at parameter-type position should resolve after Phase 1: {:?}",
        errors
    );
}

#[test]
fn nested_module_class_resolves_via_qualified_type_path() {
    let errors = resolve_errors("nested_module_class_qualified_param_type");
    assert!(
        errors.is_empty(),
        "A.B.C at parameter-type position should resolve after Phase 1: {:?}",
        errors
    );
}

#[test]
fn module_struct_enum_mixin_all_get_qualified_keys() {
    let errors = resolve_errors("module_struct_enum_mixin_qualified_keys");
    assert!(
        errors.is_empty(),
        "every nested type-level kind should get a qualified key: {:?}",
        errors
    );
}

#[test]
fn top_level_class_unaffected_by_phase_1() {
    let errors = resolve_errors("top_level_class_phase_1_unaffected");
    assert!(
        errors.is_empty(),
        "top-level Solo class should resolve as today: {:?}",
        errors
    );
}

// The `unqualified_inner_name_still_works_phase_1_additive` test
// previously pinned a TEMPORARY behaviour of Phase 1 — that
// `type_registry` carried both the un-qualified `Inner` and the
// qualified `Outer.Inner` keys for a module-nested type. Phase 4
// (inner-first scope shadowing) removed the un-qualified
// top-level entry. The new behaviour is pinned by
// `inner_first_shadowing.rs` and `module_inner_invisible_outside.rs`.
