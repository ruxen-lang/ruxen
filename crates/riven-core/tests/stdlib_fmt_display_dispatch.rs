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

    // Follow-up #2: strengthen — assert each synth fn has expected shape.
    use riven_core::hir::types::Ty;
    for expected in ["Char_fmt", "Int_fmt", "Float_fmt", "Bool_fmt", "String_fmt"] {
        let f = m
            .functions
            .iter()
            .find(|f| f.name == expected)
            .unwrap_or_else(|| panic!("synth fn `{}` missing", expected));
        assert_eq!(
            f.params.len(),
            2,
            "`{}` must have 2 params (self + fmt), got {}",
            expected,
            f.params.len()
        );
        assert_eq!(
            f.return_ty,
            Ty::Unit,
            "`{}` must have return type Unit",
            expected
        );
        assert!(
            !f.blocks[f.entry_block].instructions.is_empty(),
            "`{}` entry block must be non-empty",
            expected
        );
    }
}

/// Stage 2: a user `class Money` with `impl Display` lowers a `Money_fmt`
/// MIR function — this is the Phase C+D MVP behaviour.  Regressing this
/// would mean the impl-block lowering pathway has broken.
#[test]
fn user_impl_display_lowers_t_fmt_function() {
    // Riven syntax: `impl Display for Money` block at top level;
    // `def fmt` has no explicit `self` param — `self` is implicit.
    let src = r#"
class Money
  def init(@cents: Int)
  end
end

impl Display for Money
  def fmt(f: &mut Formatter) -> Result[(), FmtError]
    f.write_str("$")
  end
end

def main
  let _ = Money.new(100)
end
    "#;
    let m = lower(src);
    let names: Vec<&str> = m.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.iter().any(|n| *n == "Money_fmt"),
        "expected user impl-Display to lower as `Money_fmt`; got {:?}",
        names
    );
}

/// Stage 2 plumbing marker: `user_has_impl_display` is a private helper on
/// the MIR lowerer and cannot be called directly from an integration test.
/// Indirect validation is done via `user_impl_display_lowers_t_fmt_function`
/// above.  Stage 3 will add a direct behavioural assertion once the helper
/// gates an interpolation call site.
#[test]
fn user_has_impl_display_marker() {
    // Intentionally empty — validated indirectly above.
}
