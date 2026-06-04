//! End-to-end runtime pins for the numeric conversion methods added in
//! REQ3: `Int.to_f()` (widen to Float) and `Float.to_i()` (truncate to
//! Int). These compile a tiny Ruxen program, link it against the C
//! runtime, run it, and assert on stdout — proving the whole pipeline
//! (typeck → MIR → Cranelift codegen → `ruxen_int_to_f` /
//! `ruxen_float_to_i` runtime symbols) is wired correctly.
//!
//! Regression guard: before REQ3, `a.to_f()` mangled to `Int_to_f` with
//! no runtime symbol and crashed the JIT / failed AOT linking.

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
    let bin_path = tmp_dir.join(format!("{}-{}-{}.bin", basename, std::process::id(), ruxen_unique_id()));

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
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// `Int.to_f()` produces a real `Float` — the result participates in
/// float arithmetic, so `7.to_f() + 0.5` is `7.5`, not `7`.
#[test]
fn int_to_f_widens_to_float() {
    let source = "def main\n  let a: Int = 7\n  let f = a.to_f() + 0.5\n  puts \"f=#{f}\"\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "numconv_int_to_f");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("f=7.5"), "expected f=7.5, got: {}", stdout);
}

/// `Float.to_i()` truncates toward zero, yielding an `Int`.
#[test]
fn float_to_i_truncates_to_int() {
    let source = "def main\n  let g: Float = 2.9\n  let i = g.to_i()\n  puts \"i=#{i}\"\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "numconv_float_to_i");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("i=2"), "expected i=2, got: {}", stdout);
}

/// Round-trip `Int -> Float -> Int` via chained conversions. Pins the
/// exact stdout of the `809_numeric_conversions` e2e fixture.
#[test]
fn numeric_conversions_fixture_roundtrip() {
    let source = "def main\n  let a: Int = 7\n  let f = a.to_f() + 0.5\n  puts \"f=#{f}\"\n  let g: Float = 2.9\n  let i = g.to_i()\n  puts \"i=#{i}\"\n  let n: Int = 42\n  let back = n.to_f().to_i()\n  puts \"back=#{back}\"\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "numconv_roundtrip");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(stdout, "f=7.5\ni=2\nback=42\n", "fixture stdout mismatch");
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
