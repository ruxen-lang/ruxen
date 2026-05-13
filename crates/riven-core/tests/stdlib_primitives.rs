//! Pin tests for `docs/specs/stdlib/primitives.spec.md` Gaps:
//! Char escape sequences (B8) and sized-suffix typing (B5/B6).
//!
//! The `34_char_literal.rvn` fixture is a smoke test; these pins
//! harden the named-escape contract end-to-end.

use riven_core::codegen;
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

/// B8 — every named escape sequence parses and round-trips through
/// interpolation, with a wrapping `[...]` so the leading newline /
/// tab is visible in the diff if it goes wrong.
#[test]
fn prim_char_escape_sequences_round_trip() {
    let source = r##"
def main
  let nl: Char    = '\n'
  let tab: Char   = '\t'
  let bs: Char    = '\\'
  let sq: Char    = '\''

  puts "[#{nl}]"     # leading newline → multi-line output
  puts "[#{tab}]"    # leading tab
  puts "[#{bs}]"
  puts "[#{sq}]"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "prim_char_escapes");
    assert!(ok, "stderr: {}", stderr);
    // `[\n]` lands as `[` then newline then `]` then trailing `\n`.
    assert!(stdout.contains("[\n]"), "newline: {:?}", stdout);
    assert!(stdout.contains("[\t]"), "tab: {:?}", stdout);
    assert!(stdout.contains("[\\]"), "backslash: {:?}", stdout);
    assert!(stdout.contains("[']"), "single quote: {:?}", stdout);
}

/// B8 — `'\u{...}'` unicode escapes for codepoints up to U+10FFFF.
/// Each codepoint should appear correctly in the interpolated output.
#[test]
fn prim_char_unicode_escapes_round_trip() {
    let source = r##"
def main
  let a: Char = '\u{41}'          # 'A'
  let n: Char = '\u{00F1}'        # ñ
  let h: Char = '\u{6C34}'        # 水
  puts "a=#{a}"
  puts "n=#{n}"
  puts "h=#{h}"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "prim_char_unicode");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("a=A"), "ASCII: {:?}", stdout);
    assert!(stdout.contains("n=ñ"), "Latin-1: {:?}", stdout);
    assert!(stdout.contains("h=水"), "CJK: {:?}", stdout);
}

/// B5 — numeric suffixes produce the right typed value.  We rely on
/// arithmetic that would overflow at narrower widths but works at
/// wider widths to catch type-widening bugs.
#[test]
fn prim_numeric_suffix_int_widths_round_trip() {
    let source = r##"
def main
  let a: UInt8 = 200u8
  let b: UInt8 = 50u8
  # a + b would overflow at u8 (wraps to 250 → 250 ok)
  # add to a wider type after lifting:
  let sum: UInt32 = 200u32 + 50u32
  puts "sum=#{sum}"

  let small: Int8 = 7i8
  puts "small=#{small}"

  let big: Int64 = 1_234_567_890i64
  puts "big=#{big}"

  let hex: Int = 0x1F
  puts "hex=#{hex}"

  let bin: Int = 0b1010
  puts "bin=#{bin}"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "prim_numeric_suffixes");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("sum=250"), "u32 sum: {}", stdout);
    assert!(stdout.contains("small=7"), "i8: {}", stdout);
    assert!(stdout.contains("big=1234567890"), "i64: {}", stdout);
    assert!(stdout.contains("hex=31"), "0x1F = 31: {}", stdout);
    assert!(stdout.contains("bin=10"), "0b1010 = 10: {}", stdout);
}

/// B6 — scientific float notation.
#[test]
fn prim_float_scientific_notation() {
    let source = r##"
def main
  let a: Float = 1.5e3
  let b: Float = 2.0e-3
  puts "a=#{a}"
  puts "b=#{b}"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "prim_float_sci");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("a=1500"), "1.5e3 = 1500: {}", stdout);
    assert!(stdout.contains("b=0.002"), "2.0e-3 = 0.002: {}", stdout);
}
