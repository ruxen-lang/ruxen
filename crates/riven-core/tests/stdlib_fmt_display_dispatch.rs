//! Phase 2 #06.D2 — interpolation routed through Display::fmt.
//!
//! Stage 1 sanity: the synth primitive `_fmt` functions are present
//! in the MIR program for any source file (they are program-level,
//! not generated per-use).

use riven_core::diagnostics::DiagnosticLevel;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::lower_program;
use riven_core::mir::nodes::MirProgram;
use riven_core::parser::Parser;
use riven_core::typeck;

fn lower(src: &str) -> MirProgram {
    let mut lx = Lexer::new(src);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let tc = typeck::type_check(&prog);
    assert!(
        tc.diagnostics
            .iter()
            .all(|d| d.level != DiagnosticLevel::Error),
        "typecheck errors: {:?}",
        tc.diagnostics
    );
    lower_program(&tc.program, &tc.symbols).expect("mir lower")
}

/// Stage 1: `Char_fmt` / `Int_fmt` / `Float_fmt` / `Bool_fmt` /
/// `String_fmt` must appear in the MIR for every program (they are
/// unconditionally emitted at program lowering, not generated per-use).
#[test]
fn synth_primitive_fmt_functions_emitted() {
    let m = lower("def main\n  let _ = 1\nend\n");
    let names: Vec<&str> = m.functions.iter().map(|f| f.name.as_str()).collect();
    for expected in ["Char_fmt", "Int_fmt", "Float_fmt", "Bool_fmt", "String_fmt"] {
        assert!(
            names.iter().any(|n| *n == expected),
            "expected synth fn `{}` in MIR; got {:?}",
            expected,
            names
        );
    }
}
