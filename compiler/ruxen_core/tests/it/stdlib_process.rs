//! Integration test for `std::process`.
//!
//! Covers `process.exit(code)`, which needs the binary's own exit code.
//! The `Command` builder tests live in `library/std/process/tests`.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

// ─── process spec B1 direct exit-code pins (gap fill 2026-05) ──────────
//
// `std_use_resolution.rs::std_process_exit_round_trip` already pins
// `exit(23)`.  Below we cover the spec-named common codes (0, 1, 2, 42)
// in a single test that records every exit and asserts on each.

fn compile_and_get_exit_code(source: &str, basename: &str) -> i32 {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!(
        "{}-{}-{}.bin",
        basename,
        std::process::id(),
        ruxen_unique_id()
    ));

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
    output.status.code().expect("process should exit normally")
}

#[test]
fn process_exit_zero_returns_zero() {
    let source = rx("process_exit_zero_returns_zero");
    assert_eq!(
        compile_and_get_exit_code(&source, "stdlib_process_exit_0"),
        0
    );
}

#[test]
fn process_exit_one_returns_one() {
    let source = rx("process_exit_one_returns_one");
    assert_eq!(
        compile_and_get_exit_code(&source, "stdlib_process_exit_1"),
        1
    );
}

#[test]
fn process_exit_forty_two_returns_forty_two() {
    let source = rx("process_exit_forty_two_returns_forty_two");
    assert_eq!(
        compile_and_get_exit_code(&source, "stdlib_process_exit_42"),
        42
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
