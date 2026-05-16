//! Integration test for Phase 3 `std::time` module.
//!
//! Verifies that `time.now_ns()` and `time.unix_ns()` are reachable
//! through the resolver, lower to the right runtime calls, and produce
//! sensible monotonic values at runtime.

use riven_core::codegen;
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
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
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

/// `time.now_ns()` returns positive monotonic nanoseconds and a second
/// call returns a value greater than or equal to the first (monotonic
/// clock never moves backwards).
#[test]
fn time_now_ns_is_monotonic() {
    let source = rvn("time_now_ns_is_monotonic");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_monotonic");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert!(
        stdout.contains("ok"),
        "expected monotonic ordering, got: [{}]",
        stdout
    );
}

/// `time.unix_ns()` returns nanoseconds since the Unix epoch — sanity
/// check that the value is in the post-2020 range (a reasonable lower
/// bound that any modern system clock will exceed).
#[test]
fn time_unix_ns_is_post_2020() {
    let source = rvn("time_unix_ns_is_post_2020");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_unix");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert!(
        stdout.contains("ok"),
        "expected unix_ns post-2020, got: [{}]",
        stdout
    );
}
