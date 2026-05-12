//! Phase 2 #06.D2.S0 — formatter runtime C impls link and round-trip.
//!
//! Drives `rivenc` on a tiny Riven program that calls `Formatter.new()`,
//! `.write_str`, and `.buffer()` end-to-end, confirming that the six
//! `riven_fmt_formatter_*` symbols resolve at link time and behave
//! correctly at runtime.
//!
//! Uses the same `compile_and_run` helper shape as `stdlib_io.rs`.

use riven_core::codegen;
use riven_core::diagnostics::DiagnosticLevel;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
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

/// Phase 2 #06.D2.S0: `Formatter.new()` + `.write_str` + `.buffer()`
/// round-trip. After Stage 0 the six `riven_fmt_formatter_*` C symbols
/// exist; this test confirms they link and the accumulated buffer is
/// returned correctly.
#[test]
fn formatter_write_str_then_buffer_round_trips() {
    let src = r#"
def main
  let mut f = Formatter.new()
  let _ = f.write_str("hello ")
  let _ = f.write_str("world")
  println(f.buffer())
end
"#;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_runtime_round_trip");
    assert!(ok, "program exited non-zero\nstdout={stdout:?}\nstderr={stderr:?}");
    assert_eq!(stdout.trim(), "hello world");
}
