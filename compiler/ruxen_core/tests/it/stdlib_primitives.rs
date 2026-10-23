//! Pin tests for `docs/specs/stdlib/primitives.spec.md` Gaps:
//! Char escape sequences (B8) and sized-suffix typing (B5/B6).
//!
//! The `34_char_literal.rx` fixture is a smoke test; these pins
//! harden the named-escape contract end-to-end.

use crate::common;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// B8 — every named escape sequence parses and round-trips through
/// interpolation, with a wrapping `[...]` so the leading newline /
/// tab is visible in the diff if it goes wrong.
#[test]
fn prim_char_escape_sequences_round_trip() {
    let source = rx("prim_char_escape_sequences_round_trip");
    let (stdout, stderr, code) = common::compile_and_run(&source, "prim_char_escapes");
    assert_eq!(code, Some(0), "stderr: {}", stderr);
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
    let (stdout, stderr, code) = common::compile_and_run(&source, "prim_char_unicode");
    assert_eq!(code, Some(0), "stderr: {}", stderr);
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
    let (stdout, stderr, code) = common::compile_and_run(&source, "prim_numeric_suffixes");
    assert_eq!(code, Some(0), "stderr: {}", stderr);
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
    let (stdout, stderr, code) = common::compile_and_run(&source, "prim_float_sci");
    assert_eq!(code, Some(0), "stderr: {}", stderr);
    assert!(stdout.contains("a=1500"), "1.5e3 = 1500: {}", stdout);
    assert!(stdout.contains("b=0.002"), "2.0e-3 = 0.002: {}", stdout);
}
