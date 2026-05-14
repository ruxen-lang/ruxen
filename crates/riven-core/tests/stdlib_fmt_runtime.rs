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
    let src = r#"
def main
  let mut f = Formatter.new()
  let _ = f.write_char('A')
  println(f.buffer())
end
"#;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_write_char_ascii");
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
/// `tests/release-e2e/cases/070_interp_display_dispatch.rvn`.
#[test]
fn interpolation_user_impl_display_money_round_trips() {
    let src = r##"
class Money
  cents: Int

  def init(@cents: Int)
  end
end

impl Display for Money
  def fmt(f: &mut Formatter) -> Result[(), FmtError]
    let _ = f.write_str("$")
    f.write_str("#{self.cents}")
  end
end

def main
  let m = Money.new(4250)
  puts "price: #{m}"
  let n: Int = 7
  puts "count: #{n}"
  let b: Bool = true
  puts "ok: #{b}"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_user_impl_display_money");
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
    let src = r##"
def main
  let n: Int = 42
  puts "[#{n:>5}]"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_width_right_int");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[   42]\n");
}

/// Phase 2 #06.D4: left-align pads on the right.
#[test]
fn interpolation_width_left_align_pads_int() {
    let src = r##"
def main
  let n: Int = 42
  puts "[#{n:<5}]"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_width_left_int");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[42   ]\n");
}

/// Phase 2 #06.D4: center-align splits padding, extra char goes right.
#[test]
fn interpolation_width_center_align_pads_int() {
    let src = r##"
def main
  let n: Int = 42
  puts "[#{n:^6}]"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_width_center_int");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[  42  ]\n");
}

/// Phase 2 #06.D4: custom fill character with left-align.
#[test]
fn interpolation_fill_char_left_align() {
    let src = r##"
def main
  let n: Int = 7
  puts "[#{n:*<5}]"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_fill_left_int");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[7****]\n");
}

/// Phase 2 #06.D4: float precision via snprintf round-trips through
/// `Float_to_string_prec`.  `3.14159` with `.2` becomes `3.14`.
#[test]
fn interpolation_float_precision() {
    let src = r##"
def main
  let pi: Float = 3.14159
  puts "pi=#{pi:.2}"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_float_precision");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "pi=3.14\n");
}

/// Phase 2 #06.D4: string precision truncates at character boundaries.
#[test]
fn interpolation_string_precision_truncates() {
    let src = r##"
def main
  let s: String = String.from("hello world")
  puts "[#{s:.5}]"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_string_precision");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[hello]\n");
}

/// Phase 2 #06.D4: width and precision compose — precision shortens
/// the float to N decimals, width pads the result to M total chars.
#[test]
fn interpolation_width_and_precision_compose() {
    let src = r##"
def main
  let pi: Float = 3.14159
  puts "[#{pi:>8.2}]"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_width_precision_compose");
    assert!(ok, "stdout={stdout:?} stderr={stderr:?}");
    assert_eq!(stdout, "[    3.14]\n");
}

/// Phase 2 #06.D2.S1 follow-up: `Formatter.len()` returns the number of
/// bytes accumulated in the buffer so far. Captured into a binding and
/// interpolated to satisfy Riven's `println` String requirement.
#[test]
fn formatter_len_after_write_str() {
    let src = r##"
def main
  let mut f = Formatter.new()
  let _ = f.write_str("hi")
  let n = f.len()
  println("#{n}")
end
"##;
    let (stdout, stderr, ok) = compile_and_run(src, "fmt_len_after_write");
    assert!(
        ok,
        "program exited non-zero\nstdout={stdout:?}\nstderr={stderr:?}"
    );
    assert_eq!(stdout.trim(), "2");
}
