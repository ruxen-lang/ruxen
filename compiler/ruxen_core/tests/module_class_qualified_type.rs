//! Pre-flight spike for #06.95: does Ruxen's resolver support
//! `module Outer { class Inner }` with qualified-path access
//! `Outer.Inner.make(...)` from user code?
//!
//! **Status (2026-05-19): FAILS.** Tracked as the success criterion
//! for prompt `docs/prompts/v1/06_93_module_qualified_class_resolution.md`.
//! When 06.93 lands, remove the `#[ignore]` attribute and confirm
//! the test passes; that gates 06.95 Phase C.
//!
//! Current failure mode: `typecheck errors: undefined enum variant
//! `Outer.Inner``. The resolver's expression-resolution path takes
//! `Outer.Inner.make(...)` to be an enum-variant pattern because
//! that's the closest grammar match; classes declared inside a
//! module are not registered under their qualified name in
//! `type_registry`, so type-position resolution of `Outer.Inner`
//! also misses.

use ruxen_core::codegen;
use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::ast::Program;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn parse_fixture(name: &str) -> Program {
    let path = format!(
        "{}/compiler/ruxen_core/tests/fixtures/ruxen/{}.rx",
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
fn class_inside_module_resolves_via_qualified_path() {
    let program = parse_fixture("module_class_qualified_type");
    let type_result = typeck::type_check(&program);
    let errors: Vec<&Diagnostic> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "typecheck errors on `Outer.Inner.make(...)`: {:#?}",
        errors
    );

    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering");

    let bin_path = workspace_root().join("tmp/module_class_qualified_type.bin");
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path)
        .output()
        .expect("run module-class qualified-type binary");
    assert!(
        output.status.success(),
        "binary should exit 0 (Outer.Inner.make(41) == 42); status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
