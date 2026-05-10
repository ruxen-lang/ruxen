//! Phase 2 #06 (`std::fmt`) — surface tests.
//!
//! Locks in that `Display`, `Debug`, `Formatter`, and `FmtError` are
//! resolvable types/traits. Semantics (Display::fmt routing,
//! `"#{x:?}"` debug interpolation, Formatter buffering) are wired in
//! later phases; these tests guard the type-surface plumbing only.

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

fn type_errors(src: &str) -> Vec<Diagnostic> {
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

/// `Display` is resolvable as a trait, `Formatter` and `FmtError` as
/// types — a user-defined `impl Display for T` typechecks.
#[test]
fn display_trait_and_formatter_are_resolvable() {
    let src = r##"
struct Money
  cents: Int

  impl Display
    def fmt(&self, f: &mut Formatter) -> Result[(), FmtError]
      Ok(())
    end
  end
end

def main
  let _m = Money.new(100)
end
"##;
    let errs = type_errors(src);
    assert!(
        errs.is_empty(),
        "expected no type errors, got: {:#?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// `Debug` is resolvable as a trait — a user-defined `impl Debug for
/// T` with an `fmt` method typechecks.
#[test]
fn debug_trait_is_resolvable_with_fmt_method() {
    let src = r##"
struct Money
  cents: Int

  impl Debug
    def fmt(&self, f: &mut Formatter) -> Result[(), FmtError]
      Ok(())
    end
  end
end

def main
  let _m = Money.new(100)
end
"##;
    let errs = type_errors(src);
    assert!(
        errs.is_empty(),
        "expected no type errors, got: {:#?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// `Formatter::write_str` returns `Result[(), FmtError]`.
#[test]
fn formatter_write_str_returns_result_unit_fmt_error() {
    let src = r##"
struct Tag
  name: String

  impl Display
    def fmt(&self, f: &mut Formatter) -> Result[(), FmtError]
      f.write_str("tag")
    end
  end
end

def main
  let _t = Tag.new("x")
end
"##;
    let errs = type_errors(src);
    assert!(
        errs.is_empty(),
        "expected no type errors, got: {:#?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}
