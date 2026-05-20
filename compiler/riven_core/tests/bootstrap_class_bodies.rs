//! B1 pin tests for `docs/specs/system/zero_rust_stdlib_classes.spec.md`
//! — bootstrap-merge resolves class bodies.
//!
//! Pre-B1, `Resolver::resolve_with_bootstrap` ran `resolve_item` only
//! over the user program. Bootstrap programs got
//! `register_top_level_type_with_ffi` (class TYPE + lib-decl FFI
//! methods) and nothing else, which meant user-body methods on a
//! bootstrap-loaded class (`def init`, `def var poll`, `def drop`)
//! were silently dropped. The pin: a parsed bootstrap program
//! carrying a class with `def init(v: Int)` and `def get(self) ->
//! Int` produces a `HirItem::Class` whose `methods` vector contains
//! both, AND a user program can call `BootstrapBodied.new(42).get()`
//! through the typechecker without "undefined function" errors.

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::hir::nodes::HirItem;
use riven_core::lexer::Lexer;
use riven_core::parser::ast::Program;
use riven_core::parser::Parser;
use riven_core::resolve::Resolver;
use riven_core::typeck;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn parse_fixture(name: &str) -> Program {
    let path = format!(
        "{}/compiler/riven_core/tests/fixtures/riven/{}.rvn",
        workspace_root().display(),
        name
    );
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    parser.parse().expect("parse")
}

#[test]
fn bootstrap_class_body_def_init_resolves() {
    let bootstrap_program = parse_fixture("bootstrap_class_body_def_init");
    // Empty user program — we only care that the bootstrap class's
    // body methods land in the resolved HirProgram.
    let user_program = Program {
        items: vec![],
        span: bootstrap_program.span.clone(),
    };

    let resolver = Resolver::new();
    let result =
        resolver.resolve_with_bootstrap(&user_program, std::slice::from_ref(&bootstrap_program));

    let errors: Vec<&Diagnostic> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "resolver should accept a bootstrap class with `def init` + `def get` bodies; got {:?}",
        errors
    );

    // Find the HirItem::Class for BootstrapBodied.
    let class = result
        .program
        .items
        .iter()
        .find_map(|item| match item {
            HirItem::Class(c) if c.name == "BootstrapBodied" => Some(c),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "bootstrap merge should inject `class BootstrapBodied` as a HirItem so MIR \
                 lowering walks its body methods. Got items: {:?}",
                result
                    .program
                    .items
                    .iter()
                    .map(|i| format!("{:?}", std::mem::discriminant(i)))
                    .collect::<Vec<_>>()
            )
        });

    let method_names: Vec<&str> = class.methods.iter().map(|m| m.name.as_str()).collect();
    assert!(
        method_names.contains(&"init"),
        "BootstrapBodied should carry `def init` after B1; got methods {:?}",
        method_names
    );
    assert!(
        method_names.contains(&"get"),
        "BootstrapBodied should carry `def get` after B1; got methods {:?}",
        method_names
    );
}

#[test]
fn bootstrap_class_body_user_can_call_constructor() {
    // End-to-end: user program calls `BootstrapBodied.new(42).get()`.
    // Pre-B1 this fails with "undefined function 'new'" because the
    // `def init` body was dropped and the implicit constructor was
    // never synthesised.
    let bootstrap_program = parse_fixture("bootstrap_class_body_def_init");
    let user_program = parse_fixture("bootstrap_class_body_user_calls");

    let type_result =
        typeck::type_check_with_bootstrap(&user_program, std::slice::from_ref(&bootstrap_program));
    let errors: Vec<&Diagnostic> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "user code calling a bootstrap class's user-body constructor must typecheck after B1; \
         got: {:?}",
        errors
    );
}
