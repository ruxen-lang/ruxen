//! Integration tests for the `std.test` stdlib package.
//!
//! Fixture .rvn files live under `tests/fixtures/riven/test_*.rvn` per
//! the no-inline-rvn convention (feedback_no_inline_rvn_in_pin_tests.md).
//! Each test compiles + runs a fixture and asserts on captured stdout.

use riven_core::codegen;
use riven_core::diagnostics::DiagnosticLevel;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}.bin", basename));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn test_case_construct_name_and_pending_default_false() {
    let (stdout, stderr, ok) =
        compile_and_run(&rvn("test_case_construct"), "stdlib_test_case_construct");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("name=adds two numbers"), "got: {}", stdout);
    assert!(stdout.contains("pending=false"), "got: {}", stdout);
}

#[test]
fn matcher_to_eq_and_not_to_eq() {
    let (stdout, stderr, ok) =
        compile_and_run(&rvn("test_matcher_to_eq"), "stdlib_test_matcher_to_eq");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("to_eq_pass"), "got: {}", stdout);
    assert!(stdout.contains("not_to_eq_pass"), "got: {}", stdout);
}
