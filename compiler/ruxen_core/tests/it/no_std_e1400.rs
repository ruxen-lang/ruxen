//! no_std / E1400 enforcement pin tests (tier 4.04).
//!
//! Default features (no LLVM needed): exercises the `no_std::validate` pass
//! over typed HIR. A heap construction in a no_std unit → E1400; a pure
//! scalar/arithmetic unit → clean.

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::{no_std, typeck};

/// Type-check `src` with NO stdlib bootstrap (the no_std reality) and return
/// the E1400 diagnostics from the no_std validator.
fn e1400_codes(src: &str) -> Vec<String> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let tr = typeck::type_check_with_bootstrap(&program, &[]);
    no_std::validate(&tr.program)
        .into_iter()
        .filter_map(|d| d.code)
        .collect()
}

#[test]
fn pure_arithmetic_main_is_clean() {
    let src = "def main -> Int\n  let x = 40\n  let y = 2\n  x + y\nend\n";
    assert!(
        e1400_codes(src).is_empty(),
        "a pure-arithmetic no_std main must not trip E1400"
    );
}

#[test]
fn string_literal_trips_e1400() {
    let src = "def main\n  let s = \"hello\"\n  0\nend\n";
    let codes = e1400_codes(src);
    assert!(
        codes.iter().any(|c| c == "E1400"),
        "a string literal in a no_std unit must trip E1400, got: {codes:?}"
    );
}

#[test]
fn array_literal_trips_e1400() {
    let src = "def main\n  let v = [1, 2, 3]\n  0\nend\n";
    let codes = e1400_codes(src);
    assert!(
        codes.iter().any(|c| c == "E1400"),
        "an array literal in a no_std unit must trip E1400, got: {codes:?}"
    );
}

#[test]
fn scalar_ffi_call_is_clean() {
    // A no_std unit may call a scalar-only FFI without tripping E1400.
    let src = "lib \"c\"\n  def exit(code: Int32)\nend\n\
               def main\n  exit(42i32)\nend\n";
    assert!(
        e1400_codes(src).is_empty(),
        "a scalar FFI call must not trip E1400"
    );
}

#[test]
fn e1400_is_registered() {
    assert!(
        ruxen_core::diagnostics::codes::is_registered("E1400"),
        "E1400 must be in the code registry"
    );
}
