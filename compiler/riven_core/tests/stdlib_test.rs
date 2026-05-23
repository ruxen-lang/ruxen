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

#[test]
fn matcher_truthy_falsy_and_nil() {
    let (stdout, stderr, ok) =
        compile_and_run(&rvn("test_matcher_truthy_nil"), "stdlib_test_matcher_truthy_nil");
    assert!(ok, "stderr: {}", stderr);
    for token in ["truthy_pass", "falsy_pass", "not_nil_pass", "nil_pass"] {
        assert!(stdout.contains(token), "missing {token}: {}", stdout);
    }
}

#[test]
fn matcher_to_include_array_and_string() {
    let (stdout, stderr, ok) =
        compile_and_run(&rvn("test_matcher_include"), "stdlib_test_matcher_include");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("array_include_pass"), "got: {}", stdout);
    assert!(stdout.contains("string_include_pass"), "got: {}", stdout);
}

#[test]
fn runner_current_slot_roundtrip() {
    let (stdout, stderr, ok) =
        compile_and_run(&rvn("test_current_runner_slot"), "stdlib_test_current_slot");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("slot=42"), "got: {}", stdout);
}

#[test]
fn tester_capture_nongeneric_method() {
    let (stdout, stderr, ok) = compile_and_run(
        &rvn("test_tester_capture_nongeneric"),
        "stdlib_test_tester_capture_nongeneric",
    );
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("1 passed"), "got: {}", stdout);
}

#[test]
fn tester_describe_it_expect_to_eq_pass_path() {
    let (stdout, stderr, ok) = compile_and_run(
        &rvn("test_tester_describe_it_eq"),
        "stdlib_test_tester_describe_it_eq",
    );
    assert!(ok, "stderr: {}", stderr);
    // No ASSERT_FAIL_* lines should appear (both assertions pass).
    assert!(!stdout.contains("ASSERT_FAIL"), "unexpected fail: {}", stdout);
    // Runner.execute should report 2 passing cases.
    assert!(stdout.contains("2 passed"), "got: {}", stdout);
}

#[test]
fn tester_context_inherits_parent_hooks() {
    let (stdout, stderr, ok) = compile_and_run(
        &rvn("test_tester_context_hooks"),
        "stdlib_test_tester_context_hooks",
    );
    assert!(ok, "stderr: {}", stderr);
    // outer case sees only outer hooks (in order):
    let outer_idx = stdout.find("outer_case_body").expect("outer body");
    let outer_before = stdout[..outer_idx].find("outer_before").expect("outer_before before body");
    assert!(outer_before < outer_idx);
    // inner case sees outer_before THEN inner_before, then body, then outer_after:
    let inner_body_idx = stdout.find("inner_case_body").expect("inner body");
    let inner_outer_before = stdout[..inner_body_idx].rfind("outer_before").expect("outer_before for inner case");
    let inner_inner_before = stdout[..inner_body_idx].rfind("inner_before").expect("inner_before for inner case");
    assert!(inner_outer_before < inner_inner_before);
    assert!(inner_inner_before < inner_body_idx);
    // 2 passing, 0 failing, 0 pending
    assert!(stdout.contains("2 passed, 0 failed, 0 pending"), "got: {}", stdout);
}

#[test]
fn tester_summary_counts_pass_fail_pending() {
    let (stdout, stderr, _ok) = compile_and_run(
        &rvn("test_tester_xit_and_fail"),
        "stdlib_test_tester_xit_and_fail",
    );
    // Binary may exit non-zero because one test failed — that's expected.
    assert!(stdout.contains("1 passed, 1 failed, 1 pending"),
            "got stdout={} stderr={}", stdout, stderr);
}
