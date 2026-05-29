//! End-to-end pins for the universal `to_s` method (REQ4).
//!
//! Part A: scalar primitives (`Int`, `Float`, `Bool`, `Char`, `String`)
//! stringify via `to_s`, reusing the existing `ruxen_*_to_string` runtime
//! helpers. Part B (user-defined class/struct/enum `to_s` routed through
//! the Display dispatch) is pinned separately once its lowering lands.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

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
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
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
fn int_to_s_stringifies() {
    let source = "def main\n  let n: Int = 42\n  puts n.to_s()\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_int");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(stdout, "42\n", "got: {:?}", stdout);
}

#[test]
fn bool_to_s_stringifies() {
    let source = "def main\n  let b: Bool = true\n  puts b.to_s()\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_bool");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(stdout, "true\n", "got: {:?}", stdout);
}

#[test]
fn float_to_s_stringifies() {
    let source = "def main\n  let f: Float = 1.5\n  puts f.to_s()\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_float");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(stdout, "1.5\n", "got: {:?}", stdout);
}
