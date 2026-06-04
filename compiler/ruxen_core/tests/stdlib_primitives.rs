//! Pin tests for `docs/specs/stdlib/primitives.spec.md` Gaps:
//! Char escape sequences (B8) and sized-suffix typing (B5/B6).
//!
//! The `34_char_literal.rx` fixture is a smoke test; these pins
//! harden the named-escape contract end-to-end.

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

/// B8 — every named escape sequence parses and round-trips through
/// interpolation, with a wrapping `[...]` so the leading newline /
/// tab is visible in the diff if it goes wrong.
#[test]
fn prim_char_escape_sequences_round_trip() {
    let source = rx("prim_char_escape_sequences_round_trip");
    let (stdout, stderr, ok) = compile_and_run(&source, "prim_char_escapes");
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
    let source = rx("prim_char_unicode_escapes_round_trip");
    let (stdout, stderr, ok) = compile_and_run(&source, "prim_char_unicode");
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
    let source = rx("prim_numeric_suffix_int_widths_round_trip");
    let (stdout, stderr, ok) = compile_and_run(&source, "prim_numeric_suffixes");
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
    let source = rx("prim_float_scientific_notation");
    let (stdout, stderr, ok) = compile_and_run(&source, "prim_float_sci");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("a=1500"), "1.5e3 = 1500: {}", stdout);
    assert!(stdout.contains("b=0.002"), "2.0e-3 = 0.002: {}", stdout);
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
