//! #06.93 Phase 3 pin test: nested-module class method dispatches
//! to the aliased C symbol via the qualified mangled name.
//!
//! Companion to `module_class_qualified_type.rs` which exercises
//! two-level nesting (`Outer.Inner`). This one exercises THREE
//! levels (`A.B.C`) to verify the qualified-name mangling threads
//! through arbitrary nesting depth, not just the one-module case.

use riven_core::codegen;
use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::ast::Program;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::path::PathBuf;
use std::process::Command;

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
fn three_level_nested_module_dispatches_to_aliased_c_symbol() {
    let program = parse_fixture("nested_module_class_dispatches");
    let type_result = typeck::type_check(&program);
    let errors: Vec<&Diagnostic> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "typecheck errors on `A.B.C.f(21)`: {:?}",
        errors
    );

    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering");

    let bin_path = workspace_root().join("tmp/nested_module_class_dispatches.bin");
    let _ = std::fs::create_dir_all(bin_path.parent().unwrap());
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path)
        .output()
        .expect("run nested-module class dispatch binary");
    assert!(
        output.status.success(),
        "binary should exit 0 (A.B.C.f(21) == 42 via riven_test_extern_double); status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
