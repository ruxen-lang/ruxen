//! Phase 2 #06.D2.S0 — formatter runtime C impls link and round-trip.
//!
//! Drives `ruxenc` on a tiny Ruxen program that calls `Formatter.new()`,
//! `.write_str`, and `.buffer()` end-to-end, confirming that the six
//! `ruxen_fmt_formatter_*` symbols resolve at link time and behave
//! correctly at runtime.
//!
//! Uses the same `compile_and_run` helper shape as `stdlib_io.rs`.

use ruxen_core::codegen;
use ruxen_core::diagnostics::DiagnosticLevel;
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
        .filter(|d| d.level == DiagnosticLevel::Error)
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

/// Phase 2 #06.D2.S0: `Formatter.new()` + `.write_str` + `.buffer()`
/// round-trip. After Stage 0 the six `ruxen_fmt_formatter_*` C symbols
/// exist; this test confirms they link and the accumulated buffer is
/// returned correctly.
#[test]
fn formatter_write_str_then_buffer_round_trips() {
    let src = rx("formatter_write_str_then_buffer_round_trips");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_runtime_round_trip");
    assert!(
        ok,
        "program exited non-zero\nstdout={stdout:?}\nstderr={stderr:?}"
    );
    assert_eq!(stdout.trim(), "hello world");
}

/// Phase 2 #06.D2.S1 follow-up: `Formatter.write_char` with an ASCII
/// codepoint stores the byte correctly and `buffer()` returns it as a
/// one-character string.
#[test]
fn formatter_write_char_ascii_round_trips() {
    let src = rx("formatter_write_char_ascii_round_trips");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_write_char_ascii");
    assert!(
        ok,
        "program exited non-zero\nstdout={stdout:?}\nstderr={stderr:?}"
    );
    assert_eq!(stdout.trim(), "A");
}

/// Phase 2 #06.D2.S4: user `impl Display for Money` routes through
/// interpolation. Exercises the full Display dispatch path: the
/// `"#{m}"` site emits `Formatter_new` + `Money_fmt(m, fmt)` +
/// `Formatter_buffer(fmt)`, then `Money_fmt` itself uses the inner
/// primitive `"#{self.cents}"` interpolation which routes through
/// `Int_fmt`. Mirrors the release-e2e fixture at
/// `tests/release-e2e/cases/070_interp_display_dispatch.rx`.
#[test]
fn interpolation_user_impl_display_money_round_trips() {
    let src = rx("interpolation_user_impl_display_money_round_trips");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_user_impl_display_money");
    assert!(
        ok,
        "program exited non-zero\nstdout={stdout:?}\nstderr={stderr:?}"
    );
    assert_eq!(stdout, "price: $4250\ncount: 7\nok: true\n");
}

/// Phase 2 #06.D4: width + right-align (default) pads numerics on the
/// left with spaces to the requested width.
#[test]
fn interpolation_width_right_align_pads_int() {
    let src = rx("interpolation_width_right_align_pads_int");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_width_right_int");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[   42]\n");
}

/// Phase 2 #06.D4: left-align pads on the right.
#[test]
fn interpolation_width_left_align_pads_int() {
    let src = rx("interpolation_width_left_align_pads_int");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_width_left_int");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[42   ]\n");
}

/// Phase 2 #06.D4: center-align splits padding, extra char goes right.
#[test]
fn interpolation_width_center_align_pads_int() {
    let src = rx("interpolation_width_center_align_pads_int");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_width_center_int");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[  42  ]\n");
}

/// Phase 2 #06.D4: custom fill character with left-align.
#[test]
fn interpolation_fill_char_left_align() {
    let src = rx("interpolation_fill_char_left_align");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_fill_left_int");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[7****]\n");
}

/// Phase 2 #06.D4: float precision via snprintf round-trips through
/// `Float_to_string_prec`.  `3.14159` with `.2` becomes `3.14`.
#[test]
fn interpolation_float_precision() {
    let src = rx("interpolation_float_precision");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_float_precision");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "pi=3.14\n");
}

/// Phase 2 #06.D4: string precision truncates at character boundaries.
#[test]
fn interpolation_string_precision_truncates() {
    let src = rx("interpolation_string_precision_truncates");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_string_precision");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[hello]\n");
}

/// Phase 2 #06.D4: width and precision compose — precision shortens
/// the float to N decimals, width pads the result to M total chars.
#[test]
fn interpolation_width_and_precision_compose() {
    let src = rx("interpolation_width_and_precision_compose");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_width_precision_compose");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[    3.14]\n");
}

/// Phase 2 #06.D2.S1 follow-up: `Formatter.len()` returns the number of
/// bytes accumulated in the buffer so far. Captured into a binding and
/// interpolated to satisfy Ruxen's `println` String requirement.
#[test]
fn formatter_len_after_write_str() {
    let src = rx("formatter_len_after_write_str");
    let (stdout, stderr, ok) = compile_and_run(&src, "fmt_len_after_write");
    assert!(
        ok,
        "program exited non-zero\nstdout={stdout:?}\nstderr={stderr:?}"
    );
    assert_eq!(stdout.trim(), "2");
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
